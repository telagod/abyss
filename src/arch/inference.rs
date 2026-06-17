//! L0 architectural inference — fuse dictionary, naming, entry, and topology
//! signals into one `ArchFact` per indexed file.
//!
//! The pipeline runs once per index pass, immediately after temporal metrics.
//! It is read-mostly (one DB scan for files, one for the call graph already
//! built by [`super::graph`]) plus a single transactional `REPLACE INTO` on
//! `arch_facts` + `arch_modules`. No incremental updates yet — at our scale a
//! full recompute is sub-second and avoids the staleness traps incremental
//! coordinate updates would require.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::Result;
use petgraph::Direction;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use serde_json::json;

use super::{
    ArchOverride, LayerHint, build_arch_graph, classify_naming, classify_path, compute_centrality,
    compute_modules, compute_sccs, is_entry_point,
};
use crate::storage::Repository;

/// Cap on how much of each file we read for entry-point detection. Entry
/// markers (`func main`, `if __name__ == "__main__":`, etc.) are always near
/// the top — peeking deeper just slows us down on large generated files.
const ENTRY_SCAN_BYTES: usize = 8 * 1024;
/// Skip entry-point detection entirely for files this large — anything past
/// the 1 MB mark is almost certainly generated or vendored and dragging it
/// through the entry scanner is pure overhead.
const ENTRY_SKIP_BYTES: u64 = 1_000_000;

/// One row in `arch_facts`. Lives in memory between `infer_all` and the
/// `Repository::replace_arch_facts` write.
#[derive(Debug, Clone)]
pub struct ArchFact {
    pub file_id: i64,
    pub layer: String,
    pub role: String,
    pub module_id: i64,
    pub depth_from_entry: Option<u32>,
    pub centrality: f64,
    pub in_degree: u32,
    pub out_degree: u32,
    pub layer_conf: f64,
    /// JSON envelope with the per-file fusion evidence (dictionary, naming,
    /// entry flag, fusion weights). Persisted verbatim — used by the
    /// `abyss where` debug view and by the L1 eval harness later.
    pub signals: serde_json::Value,
}

/// One row in `arch_modules`. Derived from a Louvain community.
#[derive(Debug, Clone)]
pub struct ArchModuleRow {
    pub id: i64,
    pub label: String,
    pub file_count: i64,
    pub dominant_layer: String,
    pub centroid_path: String,
}

/// Run the L0 fusion over every indexed file. Order matters at the
/// per-file level (entry override → dict/naming aggregate → graph-role) but
/// each file is independent of every other file, so this loop is embarrassingly
/// parallelizable if it ever shows up as a bottleneck.
///
/// Convenience wrapper: probes the current working directory for a
/// `.code-abyss/arch.toml` override. Most callers want this. Use
/// [`infer_all_with_overrides`] if you have a different workspace in hand
/// (e.g. integration tests writing into a tempdir).
pub fn infer_all(repo: &Repository) -> Result<Vec<ArchFact>> {
    let workspace = workspace_root_for(repo);
    let overrides = workspace
        .as_deref()
        .and_then(super::override_config::load_overrides);
    infer_all_with_overrides(repo, workspace.as_deref(), overrides.as_ref())
}

/// Full-detail entry point: caller provides the workspace root (for
/// entry-point detection) and a pre-loaded override config. Either or both
/// can be `None`.
pub fn infer_all_with_overrides(
    repo: &Repository,
    workspace: Option<&Path>,
    overrides: Option<&ArchOverride>,
) -> Result<Vec<ArchFact>> {
    let workspace_root = workspace.map(|p| p.to_path_buf());

    // --- Phase 1: pull every file row in one shot so we don't ping the DB
    // once per file.
    let all_file_rows = collect_files(repo)?;

    // Drop any file the user asked us to ignore. Empty arch_facts row is
    // preferable to a half-classified one, so we filter at the source.
    let file_rows: Vec<FileRow> = if let Some(o) = overrides {
        all_file_rows
            .into_iter()
            .filter(|r| !o.is_ignored(&r.path))
            .collect()
    } else {
        all_file_rows
    };

    // --- Phase 2: build the file→file dep graph and run the three graph
    // analyses once. They all share the same `ArchGraph` to keep cost down.
    let graph = build_arch_graph(repo)?;
    let centrality = compute_centrality(&graph);
    let sccs = compute_sccs(&graph);
    let modules = compute_modules(&graph);

    // Top-20% centrality threshold — anything at or above this counts as
    // "high centrality" for the core-role rule. Computed once over the actual
    // distribution rather than hard-coding a number.
    let top_centrality_threshold = top_quintile(&centrality.pagerank);

    // BFS from every entry-point file across the reversed dep graph so we can
    // express "depth from any entry" as a single per-file number.
    let entry_set = compute_entry_set(&workspace_root, &file_rows);
    let depth_map = bfs_depth_from_entries(&graph, &entry_set);

    let mut facts: Vec<ArchFact> = Vec::with_capacity(file_rows.len());

    for row in &file_rows {
        let path = row.path.as_str();

        // --- Signal collection ---
        // Defaults first, then user overrides append on top of the dir hint
        // vec so the fusion layer sees both. User rules with higher weight
        // naturally win the layer election in `fuse_layer`.
        let mut dir_hints = classify_path(path);
        if let Some(o) = overrides {
            dir_hints.extend(o.classify_path(path));
        }
        let name_hints = classify_naming(path);
        let entry_flag = entry_set.contains(&row.file_id);

        // --- Layer fusion ---
        let (layer, layer_conf) = if entry_flag {
            ("entry".to_string(), 1.0)
        } else {
            fuse_layer(&dir_hints, &name_hints)
        };

        // --- Topology ---
        let centrality_score = centrality
            .pagerank
            .get(&row.file_id)
            .copied()
            .unwrap_or(0.0);
        let in_deg = centrality.in_degree.get(&row.file_id).copied().unwrap_or(0);
        let out_deg = centrality
            .out_degree
            .get(&row.file_id)
            .copied()
            .unwrap_or(0);
        let scc_size = sccs
            .scc_id
            .get(&row.file_id)
            .and_then(|sid| sccs.scc_size.get(sid))
            .copied()
            .unwrap_or(1);

        let role = if entry_flag {
            "entry_point".to_string()
        } else {
            fuse_role(
                in_deg,
                out_deg,
                centrality_score,
                top_centrality_threshold,
                scc_size,
            )
        };

        let module_id = modules
            .module_id
            .get(&row.file_id)
            .copied()
            .unwrap_or(-1_i64);

        let depth_from_entry = depth_map.get(&row.file_id).copied();

        let signals = json!({
            "dir": dir_hints.iter().map(|h| json!({"layer": h.layer, "weight": h.weight})).collect::<Vec<_>>(),
            "name": name_hints.iter().map(|h| json!({"layer": h.layer, "weight": h.weight})).collect::<Vec<_>>(),
            "entry": entry_flag,
        });

        facts.push(ArchFact {
            file_id: row.file_id,
            layer,
            role,
            module_id,
            depth_from_entry,
            centrality: centrality_score,
            in_degree: in_deg,
            out_degree: out_deg,
            layer_conf,
            signals,
        });
    }

    Ok(facts)
}

/// Project facts back into `arch_modules` rows by aggregating per-module
/// statistics: file count, dominant layer (mode), human-readable label, and
/// a centroid path (longest common forward-slash prefix of members).
pub fn collect_modules(facts: &[ArchFact]) -> Vec<ArchModuleRow> {
    // file_id → path lookup is needed for label derivation; the pipeline
    // hands us `ArchFact` only, so the path is derived later in
    // `replace_arch_modules`. Here we just aggregate by module_id and let
    // the repository helper enrich rows with paths via a join.
    let mut by_module: HashMap<i64, Vec<&ArchFact>> = HashMap::new();
    for f in facts {
        if f.module_id < 0 {
            continue; // file isn't in the dep graph at all
        }
        by_module.entry(f.module_id).or_default().push(f);
    }

    let mut rows: Vec<ArchModuleRow> = Vec::with_capacity(by_module.len());
    for (id, members) in by_module {
        let mut layer_counts: HashMap<&str, usize> = HashMap::new();
        for m in &members {
            *layer_counts.entry(m.layer.as_str()).or_insert(0) += 1;
        }
        let dominant_layer = layer_counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .map(|(l, _)| l.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        rows.push(ArchModuleRow {
            id,
            label: String::new(), // filled in by repo helper
            file_count: members.len() as i64,
            dominant_layer,
            centroid_path: String::new(), // filled in by repo helper
        });
    }
    // Stable order makes the test assertions less flaky.
    rows.sort_by_key(|r| r.id);
    rows
}

// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct FileRow {
    file_id: i64,
    path: String,
    language: Option<String>,
    size: i64,
}

fn collect_files(repo: &Repository) -> Result<Vec<FileRow>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare("SELECT id, path, language, size FROM files")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(FileRow {
                file_id: r.get(0)?,
                path: r.get(1)?,
                language: r.get::<_, Option<String>>(2)?,
                size: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Best-effort workspace root for resolving file paths back to disk so we
/// can peek their content during entry-point detection. The Repository
/// struct doesn't carry its db path here, so we use cwd — every CLI and MCP
/// entry point canonicalizes the workspace before opening the repo, so cwd
/// matches in practice. When it doesn't (e.g. unit-test fixtures that
/// `index_dir` into a tempdir), is_entry_point() simply gets an empty
/// content string and falls back to filename-only matching.
fn workspace_root_for(_repo: &Repository) -> Option<PathBuf> {
    std::env::current_dir().ok()
}

fn compute_entry_set(workspace: &Option<PathBuf>, file_rows: &[FileRow]) -> HashSet<i64> {
    let mut entries = HashSet::new();
    for row in file_rows {
        if row.size as u64 > ENTRY_SKIP_BYTES {
            continue;
        }
        let lang = match row.language.as_deref() {
            Some(l) if !l.is_empty() => l,
            _ => continue,
        };
        // Cheap pre-filter on basename before paying the I/O cost.
        let base = Path::new(&row.path)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let cheap_match = matches!(
            base.as_str(),
            "main.go"
                | "main.rs"
                | "main.py"
                | "__main__.py"
                | "main.ts"
                | "main.js"
                | "index.ts"
                | "index.js"
                | "index.tsx"
                | "index.jsx"
                | "main.c"
                | "main.cpp"
                | "main.cc"
                | "app.py"
        ) || row.path.contains("/bin/")
            || row.path.starts_with("bin/")
            || row.path.contains("/cmd/")
            || row.path.starts_with("cmd/")
            || base.ends_with(".java");

        if !cheap_match {
            continue;
        }

        let content_head = match workspace {
            Some(root) => read_head(&root.join(&row.path)).unwrap_or_default(),
            None => String::new(),
        };
        if is_entry_point(&row.path, &content_head, lang) {
            entries.insert(row.file_id);
        }
    }
    entries
}

fn read_head(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; ENTRY_SCAN_BYTES];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Top-20% centrality threshold. Returns the value at the 80th percentile of
/// the pagerank distribution, or 0.0 when the graph is empty. Used by the
/// role rule to flag highly-connected hubs as `core`.
fn top_quintile(pagerank: &HashMap<i64, f64>) -> f64 {
    if pagerank.is_empty() {
        return 0.0;
    }
    let mut vals: Vec<f64> = pagerank.values().copied().collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (vals.len() as f64 * 0.8).floor() as usize;
    let idx = idx.min(vals.len() - 1);
    vals[idx]
}

fn fuse_layer(dir: &[LayerHint], name: &[LayerHint]) -> (String, f64) {
    if dir.is_empty() && name.is_empty() {
        return ("unknown".to_string(), 0.0);
    }
    let mut weights: HashMap<&str, f64> = HashMap::new();
    let mut total = 0.0;
    for h in dir.iter().chain(name.iter()) {
        *weights.entry(h.layer).or_insert(0.0) += h.weight;
        total += h.weight;
    }
    let (best_layer, best_weight) = weights
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(("unknown", 0.0));
    let conf = if total > 0.0 {
        best_weight / total
    } else {
        0.0
    };
    (best_layer.to_string(), conf)
}

fn fuse_role(
    in_deg: u32,
    out_deg: u32,
    centrality: f64,
    top_threshold: f64,
    scc_size: usize,
) -> String {
    // Hub-of-cycles trumps the simpler degree rules.
    if scc_size >= 3 {
        return "bridge".to_string();
    }
    if in_deg == 0 && out_deg > 0 {
        return "entry_point".to_string();
    }
    if in_deg > 0 && out_deg == 0 {
        return "leaf".to_string();
    }
    if centrality >= top_threshold && in_deg >= 3 && out_deg >= 3 {
        return "core".to_string();
    }
    if in_deg == 0 && out_deg == 0 {
        return "orphan".to_string();
    }
    "leaf".to_string()
}

/// BFS from every entry node across the *reversed* dep graph. The reversal
/// matters: from main.go we want to follow "main calls X calls Y" by
/// traversing outgoing edges, so the input graph is already correctly
/// oriented — we walk Direction::Outgoing.
fn bfs_depth_from_entries(
    graph: &crate::arch::ArchGraph,
    entry_files: &HashSet<i64>,
) -> HashMap<i64, u32> {
    let mut depth: HashMap<i64, u32> = HashMap::new();
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();

    for &fid in entry_files {
        if let Some(&n) = graph.file_to_node.get(&fid) {
            depth.insert(fid, 0);
            queue.push_back(n);
        }
    }
    while let Some(n) = queue.pop_front() {
        let here = depth[&graph.graph[n]];
        for tgt in graph
            .graph
            .edges_directed(n, Direction::Outgoing)
            .map(|e| e.target())
            .collect::<Vec<_>>()
        {
            let fid = graph.graph[tgt];
            if let std::collections::hash_map::Entry::Vacant(slot) = depth.entry(fid) {
                slot.insert(here + 1);
                queue.push_back(tgt);
            }
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_layer_picks_highest_weight() {
        // Two dictionary hits (0.4 each) for "api" vs one for "domain" → api wins.
        let dir = vec![
            LayerHint::new("api", 0.4),
            LayerHint::new("api", 0.4),
            LayerHint::new("domain", 0.4),
        ];
        let name: Vec<LayerHint> = vec![];
        let (layer, conf) = fuse_layer(&dir, &name);
        assert_eq!(layer, "api");
        assert!(conf > 0.6, "expected >0.6 confidence, got {conf}");
    }

    #[test]
    fn fuse_layer_returns_unknown_on_empty() {
        let (layer, conf) = fuse_layer(&[], &[]);
        assert_eq!(layer, "unknown");
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn fuse_role_entry_node_classified_as_entry_point() {
        let role = fuse_role(0, 5, 0.1, 0.5, 1);
        assert_eq!(role, "entry_point");
    }

    #[test]
    fn fuse_role_leaf_classified() {
        let role = fuse_role(3, 0, 0.05, 0.5, 1);
        assert_eq!(role, "leaf");
    }

    #[test]
    fn fuse_role_core_classified() {
        let role = fuse_role(5, 5, 0.9, 0.5, 1);
        assert_eq!(role, "core");
    }

    #[test]
    fn fuse_role_bridge_classified() {
        let role = fuse_role(2, 2, 0.1, 0.5, 4);
        assert_eq!(role, "bridge");
    }

    #[test]
    fn fuse_role_orphan_classified() {
        let role = fuse_role(0, 0, 0.0, 0.5, 1);
        assert_eq!(role, "orphan");
    }

    #[test]
    fn top_quintile_empty_returns_zero() {
        let m: HashMap<i64, f64> = HashMap::new();
        assert_eq!(top_quintile(&m), 0.0);
    }

    #[test]
    fn top_quintile_picks_p80() {
        let m: HashMap<i64, f64> = (0..10).map(|i| (i, i as f64)).collect();
        // p80 of [0..9] → index 8 → value 8.0
        assert!((top_quintile(&m) - 8.0).abs() < 1e-9);
    }
}
