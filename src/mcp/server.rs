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
    /// Max results (default: 20)
    #[serde(default = "default_20")]
    pub limit: usize,
    /// Hide references resolved below this confidence (default: 0.7; 0 shows all)
    pub min_confidence: Option<f64>,
    /// Include callers from test files. Defaults to false so agents working
    /// in unfamiliar codebases see production call sites first; test callers
    /// remain reachable via `excluded_tests` count and an explicit retry.
    #[serde(default)]
    pub include_tests: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FindCallersOutput {
    pub callers: Vec<CallerItem>,
    /// Number of test-file callers omitted from `callers` because
    /// `include_tests` was false. Always 0 when `include_tests` is true.
    pub excluded_tests: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallerItem {
    pub file_path: String,
    pub symbol: String,
    pub line: u32,
    pub depth: u32,
    pub confidence: f64,
    pub is_test: bool,
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
        description = "Find all callers of a function or method. Returns who calls this symbol, from which file and line. By default test-file callers are hidden (use `include_tests: true` to see them); `excluded_tests` reports how many were dropped so the caller can decide whether to retry."
    )]
    fn find_callers(
        &self,
        Parameters(input): Parameters<FindCallersInput>,
    ) -> Json<FindCallersOutput> {
        let repo = self.repo.lock().unwrap();
        let gq = crate::graph::GraphQuery::new(&repo);
        let result = gq
            .find_callers_filtered(
                &input.symbol,
                input.limit,
                input.min_confidence.unwrap_or(0.7),
                input.include_tests,
            )
            .ok();
        let (callers, excluded_tests) = match result {
            Some(r) => (r.callers, r.excluded_tests),
            None => (Vec::new(), 0),
        };

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
                })
                .collect(),
            excluded_tests,
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
}
