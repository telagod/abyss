//! File→file weighted DiGraph + PageRank + Tarjan SCC + single-pass Louvain.
//!
//! The graph models architectural reach: each node is a file (by id) and each
//! edge `u → v` says "code in u references code in v" with weight = summed
//! confidence of those refs. Self-loops and ambiguous refs (confidence < 0.5)
//! are filtered out at build time — they're noise for centrality and a tarpit
//! for community detection.

use std::collections::HashMap;

use anyhow::Result;
use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

use crate::storage::Repository;

/// Weighted file→file dependency graph.
///
/// Node weight is the file id (so we can map back to paths after running
/// algorithms). Edge weight is the summed `refs.confidence` for that
/// (source_file, target_file) pair — a single high-confidence call counts as
/// much as several low-confidence refs, which is the trade-off we want for
/// architectural signals.
pub struct ArchGraph {
    pub graph: DiGraph<i64, f64>,
    pub file_to_node: HashMap<i64, NodeIndex>,
}

impl ArchGraph {
    /// Number of nodes (distinct files participating in the dep graph).
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges (distinct file→file pairs after dedupe).
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Lookup the file id for a given node.
    fn file_id(&self, n: NodeIndex) -> i64 {
        self.graph[n]
    }
}

/// Build the file→file weighted DiGraph from the `refs` table.
///
/// Filters: `target_file_id IS NOT NULL` (resolved refs only),
/// `source_file_id != target_file_id` (no self-loops), `confidence >= 0.5`
/// (drop the most ambiguous refs — they distort centrality scores).
pub fn build_arch_graph(repo: &Repository) -> Result<ArchGraph> {
    let conn = repo.conn();
    let mut stmt = conn.prepare(
        "SELECT source_file_id, target_file_id, SUM(confidence)
         FROM refs
         WHERE target_file_id IS NOT NULL
           AND source_file_id != target_file_id
           AND confidence >= 0.5
         GROUP BY source_file_id, target_file_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })?;

    let mut graph: DiGraph<i64, f64> = DiGraph::new();
    let mut file_to_node: HashMap<i64, NodeIndex> = HashMap::new();

    for row in rows {
        let (src, tgt, w) = row?;
        let s = *file_to_node
            .entry(src)
            .or_insert_with(|| graph.add_node(src));
        let t = *file_to_node
            .entry(tgt)
            .or_insert_with(|| graph.add_node(tgt));
        graph.add_edge(s, t, w);
    }

    Ok(ArchGraph {
        graph,
        file_to_node,
    })
}

/// PageRank + in/out degree per file.
pub struct CentralityResult {
    pub pagerank: HashMap<i64, f64>,
    pub in_degree: HashMap<i64, u32>,
    pub out_degree: HashMap<i64, u32>,
}

/// Weighted PageRank (30 iterations, damping=0.85). Edge weights are
/// normalized per source node, so a file that depends on 10 things spreads
/// its rank across them by confidence — not uniformly.
///
/// We roll our own instead of `petgraph::algo::page_rank` because petgraph's
/// implementation counts edges, not weights — for our use case (confidence
/// is the whole point) weight-aware is the honest answer.
pub fn compute_centrality(g: &ArchGraph) -> CentralityResult {
    let n = g.graph.node_count();
    let mut pagerank = HashMap::with_capacity(n);
    let mut in_degree = HashMap::with_capacity(n);
    let mut out_degree = HashMap::with_capacity(n);

    if n == 0 {
        return CentralityResult {
            pagerank,
            in_degree,
            out_degree,
        };
    }

    const DAMPING: f64 = 0.85;
    const ITERATIONS: usize = 30;

    let nodes: Vec<NodeIndex> = g.graph.node_indices().collect();
    let n_f = n as f64;
    let init = 1.0 / n_f;
    let mut ranks: HashMap<NodeIndex, f64> = nodes.iter().map(|&ni| (ni, init)).collect();

    // Precompute weighted out-sums so the inner loop is cheap.
    let out_sums: HashMap<NodeIndex, f64> = nodes
        .iter()
        .map(|&ni| {
            let sum: f64 = g
                .graph
                .edges_directed(ni, Direction::Outgoing)
                .map(|e| *e.weight())
                .sum();
            (ni, sum)
        })
        .collect();

    for _ in 0..ITERATIONS {
        // Dangling-node mass redistributes uniformly — otherwise sinks
        // permanently leak probability and the totals drift toward zero.
        let dangling_mass: f64 = nodes
            .iter()
            .filter(|&&ni| out_sums[&ni] == 0.0)
            .map(|ni| ranks[ni])
            .sum();
        let teleport = (1.0 - DAMPING) / n_f + DAMPING * dangling_mass / n_f;

        let mut new_ranks: HashMap<NodeIndex, f64> =
            nodes.iter().map(|&ni| (ni, teleport)).collect();

        for &u in &nodes {
            let out_sum = out_sums[&u];
            if out_sum == 0.0 {
                continue;
            }
            let r_u = ranks[&u];
            for e in g.graph.edges_directed(u, Direction::Outgoing) {
                let v = e.target();
                let w = *e.weight();
                *new_ranks.get_mut(&v).unwrap() += DAMPING * r_u * (w / out_sum);
            }
        }
        ranks = new_ranks;
    }

    for &ni in &nodes {
        let fid = g.file_id(ni);
        pagerank.insert(fid, ranks[&ni]);
        let ind = g.graph.edges_directed(ni, Direction::Incoming).count() as u32;
        let outd = g.graph.edges_directed(ni, Direction::Outgoing).count() as u32;
        in_degree.insert(fid, ind);
        out_degree.insert(fid, outd);
    }

    CentralityResult {
        pagerank,
        in_degree,
        out_degree,
    }
}

/// Strongly-connected components — used to flag dependency cycles
/// (files in an SCC of size > 1 reach each other directly or transitively).
pub struct SccResult {
    pub scc_id: HashMap<i64, usize>,
    pub scc_size: HashMap<usize, usize>,
}

/// Tarjan SCC via petgraph; we map each node to the index of its component
/// in the outer Vec returned by `tarjan_scc`.
pub fn compute_sccs(g: &ArchGraph) -> SccResult {
    let mut scc_id = HashMap::new();
    let mut scc_size = HashMap::new();
    let components = tarjan_scc(&g.graph);
    for (idx, comp) in components.iter().enumerate() {
        scc_size.insert(idx, comp.len());
        for &n in comp {
            scc_id.insert(g.file_id(n), idx);
        }
    }
    SccResult { scc_id, scc_size }
}

/// Community detection result.
pub struct ModuleResult {
    pub module_id: HashMap<i64, i64>,
    pub module_files: HashMap<i64, Vec<i64>>,
}

/// Tunable knobs for Louvain. Default values are the ones measured-better
/// on real codebases:
/// - `gamma=1.5` produces finer clusters than classic (`1.0`). On the abyss
///   dogfood graph this lifts module count 9 → 12 and shrinks the largest
///   community 18 → 11 files, so storage / search / temporal stop
///   collapsing into one "core infra" mega-cluster.
/// - `multi_level=false` by default. The second pass is correct Louvain —
///   it escapes local optima — but on architecture graphs (≤ a few hundred
///   nodes) it tends to RE-merge communities that gamma>1 just split apart.
///   Keep it as an opt-in knob for very large graphs (10k+ files).
#[derive(Debug, Clone, Copy)]
pub struct LouvainParams {
    /// Resolution. >1 → more communities (finer), <1 → fewer.
    pub gamma: f64,
    /// Safety cap on per-pass iteration count (each iteration is one full
    /// node-by-node sweep). Single-pass convergence is usually <10 sweeps.
    pub max_iterations: u32,
    /// Run a second Louvain pass on the supernode-collapsed graph. Off by
    /// default — only worth turning on for very large graphs where the
    /// single pass gets stuck in a local optimum.
    pub multi_level: bool,
}

impl Default for LouvainParams {
    fn default() -> Self {
        Self {
            gamma: 1.5,
            max_iterations: 30,
            multi_level: false,
        }
    }
}

/// Louvain (modularity optimization) on the *undirected* projection of the
/// file→file graph, with sensible defaults for resolution and a two-level
/// refinement pass. Returns one community per node, ids compacted to 0..K.
///
/// For tuning, see [`compute_modules_with`].
pub fn compute_modules(g: &ArchGraph) -> ModuleResult {
    compute_modules_with(g, LouvainParams::default())
}

/// Like [`compute_modules`] but exposes [`LouvainParams`] so callers can
/// tune resolution / iteration cap / multi-level on a per-call basis.
pub fn compute_modules_with(g: &ArchGraph, params: LouvainParams) -> ModuleResult {
    let n = g.graph.node_count();
    let mut module_id = HashMap::new();
    let mut module_files: HashMap<i64, Vec<i64>> = HashMap::new();

    if n == 0 {
        return ModuleResult {
            module_id,
            module_files,
        };
    }

    let nodes: Vec<NodeIndex> = g.graph.node_indices().collect();
    let node_to_idx: HashMap<NodeIndex, usize> =
        nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    // Build undirected adjacency: weight[i][j] = directed(i→j) + directed(j→i).
    let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
    for e in g.graph.edge_indices() {
        let (u, v) = g.graph.edge_endpoints(e).unwrap();
        let w = g.graph[e];
        let ui = node_to_idx[&u];
        let vi = node_to_idx[&v];
        if ui == vi {
            continue;
        }
        *adj[ui].entry(vi).or_insert(0.0) += w;
        *adj[vi].entry(ui).or_insert(0.0) += w;
    }

    // k_i = weighted degree; 2m = Σ k_i.
    let k: Vec<f64> = adj.iter().map(|m| m.values().sum::<f64>()).collect();
    let two_m: f64 = k.iter().sum();

    // Pathological: no edges at all. Each node becomes its own module.
    if two_m == 0.0 {
        for (idx, &ni) in nodes.iter().enumerate() {
            let fid = g.file_id(ni);
            module_id.insert(fid, idx as i64);
            module_files.entry(idx as i64).or_default().push(fid);
        }
        return ModuleResult {
            module_id,
            module_files,
        };
    }

    // Level 1 — Louvain pass on the original graph.
    let level1 = louvain_pass(&adj, &k, two_m, params.gamma, params.max_iterations);

    // Level 2 — collapse the level-1 communities into supernodes, run again
    // on the smaller graph to refine borderline assignments. Skipped when
    // level 1 didn't collapse (every node already its own community) or
    // when the caller disabled multi-level.
    let final_community: Vec<usize> = if params.multi_level
        && let Some(level2_map) = louvain_level2(&adj, &level1, params.gamma, params.max_iterations)
    {
        level1.iter().map(|&c| level2_map[c]).collect()
    } else {
        level1
    };

    // Compact community ids to 0..K, preserved in encounter order.
    let mut remap: HashMap<usize, i64> = HashMap::new();
    for (idx, &ni) in nodes.iter().enumerate() {
        let c = final_community[idx];
        let next_id = remap.len() as i64;
        let new_id = *remap.entry(c).or_insert(next_id);
        let fid = g.file_id(ni);
        module_id.insert(fid, new_id);
        module_files.entry(new_id).or_default().push(fid);
    }

    ModuleResult {
        module_id,
        module_files,
    }
}

/// One Louvain pass — sweep nodes until no node moves, return community
/// assignment per node (length n). `gamma` scales the null-model term in
/// the modularity-gain formula; `max_iter` is a hard sweep cap.
fn louvain_pass(
    adj: &[HashMap<usize, f64>],
    k: &[f64],
    two_m: f64,
    gamma: f64,
    max_iter: u32,
) -> Vec<usize> {
    let n = adj.len();
    let mut community: Vec<usize> = (0..n).collect();
    let mut sigma_tot: Vec<f64> = k.to_vec();

    for _ in 0..max_iter {
        let mut moved = false;
        for i in 0..n {
            let mut neighbor_weight: HashMap<usize, f64> = HashMap::new();
            for (&j, &w) in &adj[i] {
                let cj = community[j];
                *neighbor_weight.entry(cj).or_insert(0.0) += w;
            }
            let current = community[i];
            let k_i = k[i];

            sigma_tot[current] -= k_i;

            // ΔQ ∝ k_i_in_C − γ · Σ_tot[C] · k_i / 2m.
            // gamma > 1 penalises joining heavy communities, producing finer clusters.
            let mut best_c = current;
            let mut best_gain = 0.0_f64;
            for (&c, &k_i_c) in &neighbor_weight {
                let gain = k_i_c - gamma * sigma_tot[c] * k_i / two_m;
                if gain > best_gain {
                    best_gain = gain;
                    best_c = c;
                }
            }

            sigma_tot[best_c] += k_i;
            if best_c != current {
                community[i] = best_c;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    community
}

/// Level-2 refinement: collapse `level1`'s communities into supernodes,
/// run one more Louvain pass on the smaller graph, and return a map
/// `level1_community → level2_community`. Returns `None` if level 1 didn't
/// collapse the graph (no point running again).
fn louvain_level2(
    adj: &[HashMap<usize, f64>],
    level1: &[usize],
    gamma: f64,
    max_iter: u32,
) -> Option<Vec<usize>> {
    // Compact level-1 ids to 0..K so we can use them as supernode indices.
    let mut id_map: HashMap<usize, usize> = HashMap::new();
    for &c in level1 {
        let next = id_map.len();
        id_map.entry(c).or_insert(next);
    }
    let super_n = id_map.len();
    if super_n == level1.len() || super_n == 0 {
        return None; // no collapsing happened
    }

    // Aggregate edges between supernodes. Intra-community edges become
    // self-loops on the supernode (counted with the standard Louvain
    // convention: their weight enters the supernode's k but not the off-
    // diagonal adj, so they don't pull modularity gain at level 2).
    let mut super_adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); super_n];
    let mut super_k: Vec<f64> = vec![0.0; super_n];
    for (i, neighbors) in adj.iter().enumerate() {
        let ci = id_map[&level1[i]];
        for (&j, &w) in neighbors {
            let cj = id_map[&level1[j]];
            super_k[ci] += w; // k counts every incident edge weight, including intra
            if ci != cj {
                *super_adj[ci].entry(cj).or_insert(0.0) += w;
            }
        }
    }
    // Adj was double-counted (symmetric), so super_k absorbed it correctly
    // but two_m must be recomputed from the supernode k vector to match.
    let super_two_m: f64 = super_k.iter().sum();
    if super_two_m == 0.0 {
        return None;
    }

    let super_communities = louvain_pass(&super_adj, &super_k, super_two_m, gamma, max_iter);

    // Build a map indexed by RAW level-1 community id → level-2 community id.
    // The caller does `level2_map[level1[i]]`, where `level1[i]` is a raw id
    // (not compacted). Size the vec by max raw id seen.
    let raw_max = level1.iter().copied().max().unwrap_or(0);
    let mut raw_map = vec![0usize; raw_max + 1];
    for (&raw_id, &super_idx) in &id_map {
        raw_map[raw_id] = super_communities[super_idx];
    }
    Some(raw_map)
}

/// Path-B fallback: directory-prefix clustering. Unused by default — kept as
/// a deterministic baseline for V2 evals against Louvain on noisy graphs.
#[allow(dead_code)]
pub fn compute_modules_by_prefix(repo: &Repository, g: &ArchGraph) -> Result<ModuleResult> {
    let conn = repo.conn();
    let mut module_id = HashMap::new();
    let mut module_files: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut prefix_to_id: HashMap<String, i64> = HashMap::new();

    for &fid in g.file_to_node.keys() {
        let path: Option<String> = conn
            .query_row("SELECT path FROM files WHERE id = ?1", [fid], |r| r.get(0))
            .ok();
        let prefix = path
            .as_deref()
            .map(top_two_dirs)
            .unwrap_or_else(|| String::from("_"));
        let next_id = prefix_to_id.len() as i64;
        let id = *prefix_to_id.entry(prefix).or_insert(next_id);
        module_id.insert(fid, id);
        module_files.entry(id).or_default().push(fid);
    }
    Ok(ModuleResult {
        module_id,
        module_files,
    })
}

#[allow(dead_code)]
fn top_two_dirs(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        0 | 1 => String::from("_"),
        2 => parts[0].to_string(),
        _ => format!("{}/{}", parts[0], parts[1]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::storage::Repository;
    use std::collections::HashSet;
    use tempfile::TempDir;

    /// Build a Repository with hand-written refs — bypasses the indexer so
    /// tests can pin exact graph shapes.
    fn synthetic_repo(
        files: &[&str],
        refs: &[(usize, usize, f64)], // (src_idx, tgt_idx, confidence)
    ) -> (TempDir, Repository, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::new(dir.path());
        let repo = Repository::open(&cfg.db_path, cfg.model.dimensions).unwrap();
        let mut file_ids = Vec::new();
        for path in files {
            let id = repo
                .upsert_file(path, "h", Some("rust"), 0, 0, false)
                .unwrap();
            file_ids.push(id);
        }
        // Insert refs directly into the refs table (skip the resolver — we
        // already know what target_file_id we want).
        for (src, tgt, conf) in refs {
            repo.insert_ref(
                file_ids[*src],
                0,
                None,
                "x",
                None,
                Some(file_ids[*tgt]),
                None,
                "call",
                *conf,
            )
            .unwrap();
        }
        (dir, repo, file_ids)
    }

    #[test]
    fn build_arch_graph_dedupes_and_sums_confidence() {
        let (_d, repo, ids) = synthetic_repo(
            &["a.rs", "b.rs", "c.rs"],
            &[
                (0, 1, 0.9),
                (0, 1, 0.8), // same pair → should sum to 1.7
                (1, 2, 0.95),
                (0, 0, 1.0), // self-loop → ignored
                (0, 2, 0.3), // below 0.5 → filtered out
            ],
        );
        let g = build_arch_graph(&repo).unwrap();
        assert_eq!(g.node_count(), 3);
        assert_eq!(g.edge_count(), 2); // (a→b summed) + (b→c)

        // Verify the summed weight on a→b.
        let a = g.file_to_node[&ids[0]];
        let b = g.file_to_node[&ids[1]];
        let e = g.graph.find_edge(a, b).unwrap();
        let w = g.graph[e];
        assert!((w - 1.7).abs() < 1e-9, "expected 1.7, got {w}");
    }

    #[test]
    fn centrality_line_graph_b_has_positive_rank() {
        // A → B → C
        let (_d, repo, ids) =
            synthetic_repo(&["a.rs", "b.rs", "c.rs"], &[(0, 1, 1.0), (1, 2, 1.0)]);
        let g = build_arch_graph(&repo).unwrap();
        let c = compute_centrality(&g);
        assert!(c.pagerank[&ids[1]] > 0.0);
        assert_eq!(c.out_degree[&ids[0]], 1);
        assert_eq!(c.in_degree[&ids[2]], 1);
        assert_eq!(c.in_degree[&ids[0]], 0);
        assert_eq!(c.out_degree[&ids[2]], 0);
        // C is a sink with one incoming edge — should rank highest of the three.
        assert!(c.pagerank[&ids[2]] >= c.pagerank[&ids[1]]);
        assert!(c.pagerank[&ids[1]] >= c.pagerank[&ids[0]]);
        // Sums close to 1 (allowing teleport rounding).
        let sum: f64 = c.pagerank.values().sum();
        assert!((sum - 1.0).abs() < 0.05, "pagerank sum = {sum}");
    }

    #[test]
    fn scc_three_node_cycle_collapses() {
        // A → B → C → A
        let (_d, repo, ids) = synthetic_repo(
            &["a.rs", "b.rs", "c.rs"],
            &[(0, 1, 1.0), (1, 2, 1.0), (2, 0, 1.0)],
        );
        let g = build_arch_graph(&repo).unwrap();
        let s = compute_sccs(&g);
        let sa = s.scc_id[&ids[0]];
        let sb = s.scc_id[&ids[1]];
        let sc = s.scc_id[&ids[2]];
        assert_eq!(sa, sb);
        assert_eq!(sb, sc);
        assert_eq!(s.scc_size[&sa], 3);
    }

    #[test]
    fn scc_dag_each_node_own_component() {
        let (_d, repo, ids) =
            synthetic_repo(&["a.rs", "b.rs", "c.rs"], &[(0, 1, 1.0), (1, 2, 1.0)]);
        let g = build_arch_graph(&repo).unwrap();
        let s = compute_sccs(&g);
        // All three SCCs are distinct.
        let distinct: HashSet<_> = ids.iter().map(|fid| s.scc_id[fid]).collect();
        assert_eq!(distinct.len(), 3);
    }

    #[test]
    fn modules_cycle_is_one_community_classic_gamma() {
        // A 3-node cycle under classic Louvain (γ=1.0) collapses to one
        // community — every node strictly increases modularity by joining
        // its only neighbor's group.
        //
        // Default gamma was bumped to 1.5 for finer real-world clustering,
        // but the classic-mode contract still holds and the test pins it.
        let (_d, repo, _) = synthetic_repo(
            &["a.rs", "b.rs", "c.rs"],
            &[(0, 1, 1.0), (1, 2, 1.0), (2, 0, 1.0)],
        );
        let g = build_arch_graph(&repo).unwrap();
        let m = compute_modules_with(
            &g,
            LouvainParams {
                gamma: 1.0,
                max_iterations: 30,
                multi_level: false,
            },
        );
        let unique: HashSet<_> = m.module_id.values().collect();
        assert_eq!(unique.len(), 1, "cycle should be one community at γ=1");
    }

    #[test]
    fn modules_two_disconnected_pairs_two_communities() {
        // A↔B and C↔D — fully disconnected groups. Louvain must split them.
        let (_d, repo, ids) = synthetic_repo(
            &["a.rs", "b.rs", "c.rs", "d.rs"],
            &[(0, 1, 1.0), (1, 0, 1.0), (2, 3, 1.0), (3, 2, 1.0)],
        );
        let g = build_arch_graph(&repo).unwrap();
        let m = compute_modules(&g);
        assert_eq!(m.module_id[&ids[0]], m.module_id[&ids[1]]);
        assert_eq!(m.module_id[&ids[2]], m.module_id[&ids[3]]);
        assert_ne!(m.module_id[&ids[0]], m.module_id[&ids[2]]);
        assert_eq!(m.module_files.len(), 2);
    }

    #[test]
    fn empty_graph_returns_empty_results() {
        let (_d, repo, _) = synthetic_repo(&["a.rs"], &[]);
        let g = build_arch_graph(&repo).unwrap();
        assert_eq!(g.node_count(), 0); // no refs → no nodes
        let c = compute_centrality(&g);
        assert!(c.pagerank.is_empty());
        let s = compute_sccs(&g);
        assert!(s.scc_id.is_empty());
        let m = compute_modules(&g);
        assert!(m.module_id.is_empty());
    }

    #[test]
    fn directory_prefix_fallback_groups_by_top_two_dirs() {
        let (_d, repo, ids) = synthetic_repo(
            &["src/foo/a.rs", "src/foo/b.rs", "src/bar/c.rs", "tests/d.rs"],
            &[(0, 1, 1.0), (0, 2, 1.0), (1, 3, 1.0)],
        );
        let g = build_arch_graph(&repo).unwrap();
        let m = compute_modules_by_prefix(&repo, &g).unwrap();
        assert_eq!(m.module_id[&ids[0]], m.module_id[&ids[1]]);
        assert_ne!(m.module_id[&ids[0]], m.module_id[&ids[2]]);
        assert_ne!(m.module_id[&ids[0]], m.module_id[&ids[3]]);
    }
}
