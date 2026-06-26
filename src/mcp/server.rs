use std::sync::{Arc, Mutex};

use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::indexer::IndexPipeline;
use crate::search::SearchEngine;
use crate::storage::Repository;

pub struct McpServer {
    pub repo: Arc<Mutex<Repository>>,
    pub embedder: Arc<Option<Embedder>>,
    pub pipeline: Arc<IndexPipeline>,
    pub config: Config,
}

// --- Parameter types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SearchContextInput {
    /// Natural language or keyword query to search for in the codebase
    pub query: String,
    /// Maximum number of results to return (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct IndexProjectInput {}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSymbolsInput {
    /// Symbol name to search for (function, class, struct, etc.)
    pub query: String,
    /// Maximum number of results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct FileContextInput {
    /// File path, relative to the workspace (a unique path suffix also works)
    pub file: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FileContextOutput {
    /// Whether the file was found in the index
    pub found: bool,
    /// Full context document (same shape as `abyss context --json`)
    pub context: serde_json::Value,
}

// --- Output types ---

#[derive(Serialize, schemars::JsonSchema)]
pub struct SearchContextOutput {
    pub results: Vec<SearchResultItem>,
    pub total: usize,
    /// What this binary actually delivered: "fulltext" in slim builds (no
    /// embedder), "semantic+fulltext" in `--features semantic` builds. Lets
    /// callers tell whether they got vector-similarity matching or only
    /// keyword/symbol matching — the tool description alone cannot.
    pub precision_mode: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SearchResultItem {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub content: String,
    pub kind: String,
    pub scope: Option<String>,
    pub score: f64,
    pub match_sources: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct IndexProjectOutput {
    pub files: u64,
    pub chunks: u64,
    pub symbols: u64,
    pub duration_ms: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GetSymbolsOutput {
    pub symbols: Vec<SymbolItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SymbolItem {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line: u32,
    pub scope: Option<String>,
}

// --- Graph tool types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct FindCallersInput {
    /// Symbol name to find callers of
    pub symbol: String,
    /// Max results (default: 20). Pass `0` for unlimited (capped at 50_000
    /// internally so a hot framework primitive can't OOM the agent).
    #[serde(default = "default_20")]
    pub limit: usize,
    /// Hide references resolved below this confidence (default: 0.7; 0 shows all)
    pub min_confidence: Option<f64>,
    /// Include callers from test files. Defaults to false so agents working
    /// in unfamiliar codebases see production call sites first; test callers
    /// remain reachable via `excluded_tests` count and an explicit retry.
    #[serde(default)]
    pub include_tests: bool,
    /// Which edge kinds count as callers. Valid entries: `call`,
    /// `field_access`, `type_ref`, `inherit`. Default is all four — for
    /// "who depends on X" the agent almost always wants type-position users
    /// (annotations, generics, `extends`) AND inheritance users (every
    /// subclass) alongside the invocation users. To recover the legacy
    /// invocation-only behaviour, pass `["call","field_access"]`; to look
    /// only at type users, pass `["type_ref"]`; for "every subclass", pass
    /// `["inherit"]`. Empty array is treated as default.
    #[serde(default)]
    pub kinds: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FindCallersOutput {
    pub callers: Vec<CallerItem>,
    /// Number of test-file callers omitted from `callers` because
    /// `include_tests` was false. Always 0 when `include_tests` is true.
    pub excluded_tests: usize,
    /// Total visible callers in the index (above `min_confidence`, filtered
    /// to the requested `kinds`, with test callers excluded when
    /// `include_tests=false`). Lets the agent tell "all callers" from "the
    /// first N of M" without re-querying.
    #[serde(default)]
    pub total_available: usize,
    /// True when `callers.len() < total_available` — the agent should
    /// consider raising `limit` (or passing `0`) to see the full set.
    #[serde(default)]
    pub was_capped: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallerItem {
    pub file_path: String,
    pub symbol: String,
    pub line: u32,
    pub depth: u32,
    pub confidence: f64,
    pub is_test: bool,
    /// Ref kind: `call`, `field_access`, or `type_ref`. Lets the agent
    /// distinguish "X invokes this" from "X annotates with this type"
    /// without re-querying.
    pub kind: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ImpactInput {
    /// Symbol name to analyze impact for
    pub symbol: String,
    /// Max transitive depth (default: 3)
    pub depth: Option<u32>,
    /// Exclude references resolved below this confidence (default: 0.7; 0 includes all)
    pub min_confidence: Option<f64>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ImpactOutput {
    pub target: String,
    pub direct_callers: Vec<CallerItem>,
    pub transitive_callers: Vec<CallerItem>,
    pub affected_tests: Vec<CallerItem>,
    pub uncovered_paths: Vec<String>,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

fn default_20() -> usize {
    20
}
fn default_30() -> usize {
    30
}

/// Map a wire-level `kinds` array onto the closed [`CallerKindFilter`] enum.
/// Returns `None` for empty / unknown inputs so the caller can fall back to
/// `Both` (the safe superset) rather than silently dropping results.
///
/// The mapping is intentionally narrow: four filter shapes (CallsOnly /
/// TypesOnly / InheritsOnly / Both) is enough until evidence says otherwise,
/// and a strict classifier keeps a malformed `kinds: ["typeref"]` (typo) from
/// hiding a caller list — we just return `Both` instead.
///
/// Mixed kinds collapse to `Both` (the superset): anything other than a
/// pure single-kind selection is wider than any of the restricted variants,
/// so widening to Both honours intent without inventing a new filter.
fn classify_kinds(kinds: &[String]) -> Option<crate::graph::CallerKindFilter> {
    use crate::graph::CallerKindFilter;
    if kinds.is_empty() {
        return None;
    }
    let mut has_call = false;
    let mut has_field = false;
    let mut has_type = false;
    let mut has_inherit = false;
    for k in kinds {
        match k.as_str() {
            "call" => has_call = true,
            "field_access" => has_field = true,
            "type_ref" => has_type = true,
            "inherit" => has_inherit = true,
            _ => return None, // unknown kind → caller picks default
        }
    }
    let invoke = has_call || has_field;
    Some(match (invoke, has_type, has_inherit) {
        (true, false, false) => CallerKindFilter::CallsOnly,
        (false, true, false) => CallerKindFilter::TypesOnly,
        (false, false, true) => CallerKindFilter::InheritsOnly,
        (false, false, false) => return None,
        // Any combination of two or more kinds → caller wants more than one
        // axis, fall back to the agent superset.
        _ => CallerKindFilter::Both,
    })
}

// --- Temporal tool types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct CodeMapInput {
    /// Number of days for hotspot analysis (default: 30)
    #[serde(default = "default_30")]
    pub days: usize,
    /// Max hotspots to return (default: 15)
    #[serde(default = "default_15")]
    pub limit: usize,
}

fn default_15() -> usize {
    15
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CodeMapOutput {
    pub hotspots: Vec<HotspotMcpItem>,
    pub coupled_files: Vec<CoupledPairItem>,
    pub total_files: u64,
    pub total_refs: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct HotspotMcpItem {
    pub file_path: String,
    pub change_count: u32,
    pub complexity: f64,
    pub hotspot_score: f64,
    pub max_func_lines: u32,
    pub unique_authors: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CoupledPairItem {
    pub file_a: String,
    pub file_b: String,
    pub co_changes: u32,
    pub coupling_score: f64,
}

// --- Arch map tool types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ArchMapInput {}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ArchMapOutput {
    pub total_files: u64,
    pub modules: Vec<ArchModuleItem>,
    pub layers: Vec<LabelCount>,
    pub roles: Vec<LabelCount>,
    pub top_centrality: Vec<CentralityItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ArchModuleItem {
    pub id: i64,
    pub label: String,
    pub files: u64,
    pub dominant_layer: String,
    pub centroid_path: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct LabelCount {
    pub label: String,
    pub count: u64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CentralityItem {
    pub path: String,
    pub centrality: f64,
}

// --- Evolution tool types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct EvolutionInput {
    /// File path (relative to workspace)
    pub file: String,
    /// Symbol name (function/method). If omitted, traces file-level history.
    pub symbol: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct EvolutionOutput {
    pub file_path: String,
    pub symbol: Option<String>,
    pub commits: Vec<EvCommit>,
    pub coupled_files: Vec<CoupledPairItem>,
    pub churn_rate: f64,
    pub unique_authors: u32,
    pub total_changes: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct EvCommit {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

// --- Proxy gain tool types ---

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ProxyGainInput {
    /// Number of days to look back (default: 30)
    #[serde(default)]
    pub days: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProxyGainOutput {
    pub total_commands: u64,
    pub total_raw_tokens: u64,
    pub total_filtered_tokens: u64,
    pub total_saved_tokens: u64,
    pub avg_savings_pct: f64,
    pub top_commands: Vec<ProxyGainCommand>,
    pub daily: Vec<ProxyGainDay>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProxyGainCommand {
    pub command: String,
    pub saved_tokens: u64,
    pub savings_pct: f64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProxyGainDay {
    pub date: String,
    pub commands: u64,
    pub saved_tokens: u64,
    pub raw_tokens: u64,
}

// --- Tool router ---

#[tool_router(server_handler)]
impl McpServer {
    #[tool(
        name = "search_context",
        description = "Search the codebase by symbol and full-text. Semantic (vector) similarity is available only when the binary was built with `--features semantic` — check the `precision_mode` field on the response to see what was actually used. Returns relevant code chunks with file paths, line numbers, and content."
    )]
    fn search_context(
        &self,
        Parameters(input): Parameters<SearchContextInput>,
    ) -> Json<SearchContextOutput> {
        let repo = self.repo.lock().unwrap();
        let embedder_ref = self.embedder.as_ref().as_ref();
        let engine = SearchEngine::new(&repo, embedder_ref);

        let results = engine.search(&input.query, input.limit).unwrap_or_default();
        let total = results.len();

        // Honest mode tag: slim builds cannot run the embedder (it's an
        // `unreachable!()` stub) so they only deliver fulltext/symbol matches.
        // Semantic builds with a successfully-loaded model deliver both.
        let precision_mode = if embedder_ref.is_some() {
            "semantic+fulltext".to_string()
        } else {
            "fulltext".to_string()
        };

        Json(SearchContextOutput {
            results: results
                .into_iter()
                .map(|r| SearchResultItem {
                    file_path: r.file_path,
                    start_line: r.start_line,
                    end_line: r.end_line,
                    content: r.content,
                    kind: r.kind,
                    scope: r.scope,
                    score: r.score,
                    match_sources: r.match_sources,
                })
                .collect(),
            total,
            precision_mode,
        })
    }

    #[tool(
        name = "index_project",
        description = "Rebuild the code index for the current workspace. Fast structural index (symbols + full-text), typically completes in seconds."
    )]
    fn index_project(
        &self,
        Parameters(_input): Parameters<IndexProjectInput>,
    ) -> Json<IndexProjectOutput> {
        let repo = self.repo.lock().unwrap();
        let stats = self.pipeline.run_structural(&repo).unwrap_or_default();

        Json(IndexProjectOutput {
            files: stats.total_files,
            chunks: stats.total_chunks,
            symbols: stats.total_symbols,
            duration_ms: stats.duration_ms,
        })
    }

    #[tool(
        name = "get_symbols",
        description = "Search for symbol definitions (functions, classes, structs, etc.) by name. Supports exact, prefix, and substring matching."
    )]
    fn get_symbols(
        &self,
        Parameters(input): Parameters<GetSymbolsInput>,
    ) -> Json<GetSymbolsOutput> {
        let repo = self.repo.lock().unwrap();
        let results =
            crate::search::symbol::search(&repo, &input.query, input.limit).unwrap_or_default();

        Json(GetSymbolsOutput {
            symbols: results
                .into_iter()
                .map(|r| SymbolItem {
                    name: r.name,
                    kind: r.kind,
                    file_path: r.file_path,
                    line: r.line,
                    scope: r.scope,
                })
                .collect(),
        })
    }

    #[tool(
        name = "file_context",
        description = "Pre-edit context for a file: every symbol with external callers (confidence-tagged), possible low-confidence callers, dependencies, hotspot score, and change-coupled files. Call this BEFORE modifying a file."
    )]
    fn file_context(
        &self,
        Parameters(input): Parameters<FileContextInput>,
    ) -> Json<FileContextOutput> {
        let repo = self.repo.lock().unwrap();
        match crate::context::build_file_context(&repo, &input.file)
            .ok()
            .flatten()
        {
            Some(ctx) => Json(FileContextOutput {
                found: true,
                context: ctx,
            }),
            None => Json(FileContextOutput {
                found: false,
                context: serde_json::json!({ "file": input.file }),
            }),
        }
    }

    #[tool(
        name = "find_callers",
        description = "Find who depends on a symbol. Returns invocation callers, type-position users (interface implementers, generic instantiations, `extends`), AND inheritance users (every subclass of a base class) — for an agent asking 'who uses this' all three matter. Default kind set is [call, field_access, type_ref, inherit]; pass `kinds: [\"call\",\"field_access\"]` for invocation-only, `kinds: [\"type_ref\"]` for type users only, or `kinds: [\"inherit\"]` for 'every subclass of this base'. Each row carries `kind` so the agent can tell them apart. `limit: 0` means unlimited (50_000 internal cap). Output reports `total_available` + `was_capped` so the agent knows when there's more to see. Test-file callers are hidden by default (set `include_tests: true` to include them; `excluded_tests` reports how many were dropped)."
    )]
    fn find_callers(
        &self,
        Parameters(input): Parameters<FindCallersInput>,
    ) -> Json<FindCallersOutput> {
        use crate::graph::CallerKindFilter;
        let repo = self.repo.lock().unwrap();
        let gq = crate::graph::GraphQuery::new(&repo);

        // Map the wire-level `kinds` array to the closed filter enum. The
        // SQL layer (`find_callers_of_kinds`) is happy to take an arbitrary
        // `&[&str]` but the rest of the codebase keeps that surface narrow
        // on purpose — four filter shapes is enough until evidence says
        // otherwise. Unknown / mixed inputs fall back to `Both` (the safe
        // superset) so a malformed agent call never hides results silently.
        let kind_filter = classify_kinds(&input.kinds).unwrap_or(CallerKindFilter::Both);

        // `limit: 0` → unlimited, capped at 50_000 internally (B3).
        const UNLIMITED_CAP: usize = 50_000;
        let effective_limit = if input.limit == 0 {
            UNLIMITED_CAP
        } else {
            input.limit
        };
        let min_confidence = input.min_confidence.unwrap_or(0.7);

        let result = gq
            .find_callers_filtered_kinds(
                &input.symbol,
                effective_limit,
                min_confidence,
                input.include_tests,
                kind_filter,
            )
            .ok();
        let (callers, excluded_tests) = match result {
            Some(r) => (r.callers, r.excluded_tests),
            None => (Vec::new(), 0),
        };
        // Total visible callers for the "showing N of M" agent hint (B3).
        // Falls back to `callers.len()` if the count query errors so the
        // tool never fails open.
        let total_available = repo
            .count_callers_at(
                &input.symbol,
                kind_filter.as_slice(),
                min_confidence,
                input.include_tests,
            )
            .unwrap_or(callers.len());
        let was_capped = callers.len() < total_available;

        Json(FindCallersOutput {
            callers: callers
                .into_iter()
                .map(|c| CallerItem {
                    file_path: c.file_path,
                    symbol: c.symbol,
                    line: c.line,
                    depth: c.depth,
                    confidence: c.confidence,
                    is_test: c.is_test,
                    kind: c.kind,
                })
                .collect(),
            excluded_tests,
            total_available,
            was_capped,
        })
    }

    #[tool(
        name = "impact_analysis",
        description = "Analyze the blast radius of changing a symbol. Returns direct/transitive callers, affected tests, uncovered call paths, and a risk score (0-10)."
    )]
    fn impact_analysis(&self, Parameters(input): Parameters<ImpactInput>) -> Json<ImpactOutput> {
        let repo = self.repo.lock().unwrap();
        let gq = crate::graph::GraphQuery::new(&repo);
        let result = gq
            .impact_analysis(
                &input.symbol,
                input.depth.unwrap_or(3),
                input.min_confidence.unwrap_or(0.7),
            )
            .unwrap();

        Json(ImpactOutput {
            target: result.target,
            direct_callers: result
                .direct_callers
                .into_iter()
                .map(|c| CallerItem {
                    file_path: c.file_path,
                    symbol: c.symbol,
                    line: c.line,
                    depth: c.depth,
                    confidence: c.confidence,
                    is_test: c.is_test,
                    kind: c.kind,
                })
                .collect(),
            transitive_callers: result
                .transitive_callers
                .into_iter()
                .map(|c| CallerItem {
                    file_path: c.file_path,
                    symbol: c.symbol,
                    line: c.line,
                    depth: c.depth,
                    confidence: c.confidence,
                    is_test: c.is_test,
                    kind: c.kind,
                })
                .collect(),
            affected_tests: result
                .affected_tests
                .into_iter()
                .map(|c| CallerItem {
                    file_path: c.file_path,
                    symbol: c.symbol,
                    line: c.line,
                    depth: c.depth,
                    confidence: c.confidence,
                    is_test: c.is_test,
                    kind: c.kind,
                })
                .collect(),
            uncovered_paths: result.uncovered_paths,
            risk_score: result.risk_score,
            risk_factors: result.risk_factors,
        })
    }

    #[tool(
        name = "code_map",
        description = "Get a high-level codebase map: hotspots (high churn × complexity), change-coupled file pairs, and summary stats. Use to understand which areas are most risky or active."
    )]
    fn code_map(&self, Parameters(input): Parameters<CodeMapInput>) -> Json<CodeMapOutput> {
        let repo = self.repo.lock().unwrap();
        let hotspots =
            crate::temporal::hotspot::top_hotspots(&repo, input.limit).unwrap_or_default();
        let coupled =
            crate::temporal::coupling::top_coupled(&repo, input.limit).unwrap_or_default();
        let total_files = repo.file_count().unwrap_or(0) as u64;
        let total_refs = repo.ref_count().unwrap_or(0) as u64;

        Json(CodeMapOutput {
            hotspots: hotspots
                .into_iter()
                .map(|h| HotspotMcpItem {
                    file_path: h.file_path,
                    change_count: h.change_count,
                    complexity: h.complexity,
                    hotspot_score: h.hotspot_score,
                    max_func_lines: h.max_func_lines,
                    unique_authors: h.unique_authors,
                })
                .collect(),
            coupled_files: coupled
                .into_iter()
                .map(|c| CoupledPairItem {
                    file_a: c.file_a,
                    file_b: c.file_b,
                    co_changes: c.co_changes,
                    coupling_score: c.coupling_score,
                })
                .collect(),
            total_files,
            total_refs,
        })
    }

    #[tool(
        name = "arch_map",
        description = "L0 architectural map of the codebase: per-layer / per-role file counts, Louvain modules with labels and dominant layers, and the top-N files by PageRank centrality. Use to answer 'what does this codebase look like overall?' without crawling individual files."
    )]
    fn arch_map(&self, Parameters(_input): Parameters<ArchMapInput>) -> Json<ArchMapOutput> {
        let repo = self.repo.lock().unwrap();
        let total_files = repo.file_count().unwrap_or(0) as u64;
        let modules = repo.list_arch_modules().unwrap_or_default();
        let layers = repo.arch_layer_counts().unwrap_or_default();
        let roles = repo.arch_role_counts().unwrap_or_default();
        let top_central = repo.arch_top_centrality(10).unwrap_or_default();

        Json(ArchMapOutput {
            total_files,
            modules: modules
                .into_iter()
                .map(|m| ArchModuleItem {
                    id: m.id,
                    label: m.label,
                    files: m.file_count as u64,
                    dominant_layer: m.dominant_layer,
                    centroid_path: m.centroid_path,
                })
                .collect(),
            layers: layers
                .into_iter()
                .map(|(k, v)| LabelCount {
                    label: k,
                    count: v as u64,
                })
                .collect(),
            roles: roles
                .into_iter()
                .map(|(k, v)| LabelCount {
                    label: k,
                    count: v as u64,
                })
                .collect(),
            top_centrality: top_central
                .into_iter()
                .map(|(path, c)| CentralityItem {
                    path,
                    centrality: c,
                })
                .collect(),
        })
    }

    #[tool(
        name = "evolution",
        description = "Trace the history of a file or specific function/symbol. Returns commits, change-coupled files, churn rate, and authors. Use to understand why code looks the way it does."
    )]
    fn evolution(&self, Parameters(input): Parameters<EvolutionInput>) -> Json<EvolutionOutput> {
        let repo = self.repo.lock().unwrap();
        let result = match crate::temporal::evolution::trace_evolution(
            &self.config.workspace,
            &repo,
            &input.file,
            input.symbol.as_deref(),
        ) {
            Ok(r) => r,
            Err(_) => {
                return Json(EvolutionOutput {
                    file_path: input.file.clone(),
                    symbol: input.symbol.clone(),
                    commits: Vec::new(),
                    coupled_files: Vec::new(),
                    churn_rate: 0.0,
                    unique_authors: 0,
                    total_changes: 0,
                });
            }
        };

        Json(EvolutionOutput {
            file_path: result.file_path,
            symbol: result.symbol,
            commits: result
                .commits
                .into_iter()
                .map(|c| EvCommit {
                    hash: c.hash,
                    author: c.author,
                    date: c.date,
                    message: c.message,
                })
                .collect(),
            coupled_files: result
                .coupled_files
                .into_iter()
                .map(|c| CoupledPairItem {
                    file_a: c.path.clone(),
                    file_b: String::new(),
                    co_changes: c.co_changes,
                    coupling_score: c.coupling_score,
                })
                .collect(),
            churn_rate: result.churn_rate,
            unique_authors: result.unique_authors,
            total_changes: result.total_changes,
        })
    }

    #[tool(
        name = "proxy_gain",
        description = "Show token savings from proxied commands. Returns total tokens saved, average compression ratio, top commands by savings, and daily breakdown. Use to measure how much context budget the proxy is recovering."
    )]
    fn proxy_gain(&self, Parameters(input): Parameters<ProxyGainInput>) -> Json<ProxyGainOutput> {
        let repo = self.repo.lock().unwrap();
        let conn = repo.conn();
        let days = if input.days == 0 { 30 } else { input.days };
        let summary = match crate::proxy::tracking::gain_summary(conn, days) {
            Ok(s) => s,
            Err(_) => {
                return Json(ProxyGainOutput {
                    total_commands: 0,
                    total_raw_tokens: 0,
                    total_filtered_tokens: 0,
                    total_saved_tokens: 0,
                    avg_savings_pct: 0.0,
                    top_commands: Vec::new(),
                    daily: Vec::new(),
                });
            }
        };

        Json(ProxyGainOutput {
            total_commands: summary.total_commands,
            total_raw_tokens: summary.total_raw_tokens,
            total_filtered_tokens: summary.total_filtered_tokens,
            total_saved_tokens: summary.total_saved_tokens,
            avg_savings_pct: summary.avg_savings_pct,
            top_commands: summary
                .top_commands
                .into_iter()
                .map(|(cmd, saved, pct)| ProxyGainCommand {
                    command: cmd,
                    saved_tokens: saved,
                    savings_pct: pct,
                })
                .collect(),
            daily: summary
                .daily
                .into_iter()
                .map(|d| ProxyGainDay {
                    date: d.date,
                    commands: d.commands,
                    saved_tokens: d.saved,
                    raw_tokens: d.raw,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod kinds_tests {
    //! Pins the `find_callers` `kinds` contract surfaced over MCP.
    //!
    //! Three filter shapes are valid; everything else falls back to the
    //! safe superset (None → `Both`) so an agent typo like `"typeref"`
    //! can never silently hide results.
    use super::classify_kinds;
    use crate::graph::CallerKindFilter;

    fn k(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn empty_kinds_returns_none_so_caller_picks_default() {
        assert!(classify_kinds(&[]).is_none());
    }

    #[test]
    fn call_only_maps_to_calls_only() {
        assert_eq!(
            classify_kinds(&k(&["call"])),
            Some(CallerKindFilter::CallsOnly)
        );
        assert_eq!(
            classify_kinds(&k(&["call", "field_access"])),
            Some(CallerKindFilter::CallsOnly)
        );
    }

    #[test]
    fn type_ref_only_maps_to_types_only() {
        assert_eq!(
            classify_kinds(&k(&["type_ref"])),
            Some(CallerKindFilter::TypesOnly)
        );
    }

    #[test]
    fn mixed_call_and_type_maps_to_both() {
        assert_eq!(
            classify_kinds(&k(&["call", "type_ref"])),
            Some(CallerKindFilter::Both)
        );
        assert_eq!(
            classify_kinds(&k(&["call", "field_access", "type_ref"])),
            Some(CallerKindFilter::Both)
        );
    }

    #[test]
    fn unknown_kind_falls_back_to_default() {
        // Typo (`typeref` no underscore) or unrecognised future kind →
        // caller picks default rather than us silently dropping a request.
        assert!(classify_kinds(&k(&["typeref"])).is_none());
        assert!(classify_kinds(&k(&["call", "wat"])).is_none());
    }

    #[test]
    fn inherit_only_maps_to_inherits_only() {
        // Django Model dogfood (2026-06-17): the `inherit` axis needs its
        // own filter so an agent can ask "every subclass of Model" without
        // also pulling in unrelated call sites.
        assert_eq!(
            classify_kinds(&k(&["inherit"])),
            Some(CallerKindFilter::InheritsOnly)
        );
    }

    #[test]
    fn mixed_with_inherit_collapses_to_both() {
        // Any combination of two or more kinds collapses to the agent
        // superset rather than inventing a new partial filter.
        assert_eq!(
            classify_kinds(&k(&["call", "inherit"])),
            Some(CallerKindFilter::Both)
        );
        assert_eq!(
            classify_kinds(&k(&["type_ref", "inherit"])),
            Some(CallerKindFilter::Both)
        );
        assert_eq!(
            classify_kinds(&k(&["call", "field_access", "type_ref", "inherit"])),
            Some(CallerKindFilter::Both)
        );
    }
}
