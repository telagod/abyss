use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

use code_abyss::config::Config;
#[cfg(feature = "semantic")]
use code_abyss::embedding::Embedder;
use code_abyss::indexer::IndexPipeline;
use code_abyss::mcp::McpServer;
use code_abyss::storage::Repository;
use rmcp::ServiceExt;

#[derive(Parser)]
#[command(
    name = "abyss",
    version,
    about = "Code relationship graph and temporal intelligence"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true, default_value = ".")]
    workspace: PathBuf,

    #[arg(short, long, global = true)]
    db: Option<PathBuf>,

    #[arg(long, global = true)]
    model: Option<String>,

    /// Output JSON (for AI consumption)
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Fast structural index (symbols + fulltext, no embedding). Seconds.
    Index {
        /// Skip workspace safety checks (banned paths, file-count breaker)
        #[arg(long)]
        force: bool,
        /// Max files to index before aborting (0 = no limit; default 50000 without .git)
        #[arg(long)]
        max_files: Option<u64>,
        /// Extract refs from machine-generated files too (DO NOT EDIT markers);
        /// default keeps their symbols but skips their high-noise call edges
        #[arg(long)]
        index_generated: bool,
    },
    /// Generate embeddings for semantic search. Slow, run after `index`.
    #[cfg(feature = "semantic")]
    Embed,
    /// Full index + embed in one shot
    #[cfg(feature = "semantic")]
    IndexAll {
        #[arg(long)]
        watch: bool,
    },
    /// Search the code index (symbols + fulltext + semantic if available)
    Search {
        query: String,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Find all callers of a symbol
    Callers {
        symbol: String,
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Hide references resolved below this confidence (0 shows everything)
        #[arg(long, default_value = "0.7")]
        min_confidence: f64,
    },
    /// Analyze blast radius of changing a symbol
    Impact {
        symbol: String,
        #[arg(short, long, default_value = "3")]
        depth: u32,
        /// Exclude references resolved below this confidence (0 includes everything)
        #[arg(long, default_value = "0.7")]
        min_confidence: f64,
    },
    /// Trace evolution of a file or symbol
    History {
        file: String,
        #[arg(short, long)]
        symbol: Option<String>,
    },
    /// Get full context for a file: all functions, their callers, risk, and related files
    Context {
        /// File path (relative to workspace)
        file: String,
    },
    /// Show codebase map: hotspots, coupling, risk areas
    Map {
        #[arg(long, default_value = "15")]
        limit: usize,
    },
    /// Show the L0 architectural coordinates for a file (layer/module/role/topology)
    Where {
        /// File path (relative to workspace, suffix-matchable)
        file: String,
    },
    /// Show index statistics
    Stats,
    /// Agent hook entry points (read tool-call JSON from stdin)
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Install abyss hooks into an agent host's settings (idempotent).
    /// Supported hosts: claude
    Attach {
        /// Agent host (currently: claude)
        host: String,
        /// Write to <cwd>/.claude/settings.json instead of $HOME
        #[arg(long)]
        local: bool,
    },
    /// Run as MCP server (stdio transport)
    Mcp,
}

#[derive(Subcommand)]
enum HookAction {
    /// Pre-edit guard: emit a `<abyss-card>` system-reminder block on stderr
    /// for the file referenced in the tool-call JSON on stdin. Read-only —
    /// never re-indexes (use post-edit for that).
    PreEdit,
    /// Post-edit: incrementally refresh the index
    PostEdit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Hooks share stderr with the agent — keep it for actionable warnings only.
    let default_level = if matches!(cli.command, Commands::Hook { .. }) {
        "warn"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level)),
        )
        .with_writer(std::io::stderr)
        .init();
    let workspace = std::fs::canonicalize(&cli.workspace)?;
    let mut config = Config::new(&workspace);

    if let Some(db) = cli.db {
        config.db_path = db;
    }
    if let Some(model) = cli.model {
        config.model.model_id = model;
    }

    let json = cli.json;

    match cli.command {
        Commands::Index {
            force,
            max_files,
            index_generated,
        } => cmd_index(config, json, force, max_files, index_generated),
        #[cfg(feature = "semantic")]
        Commands::Embed => cmd_embed(config),
        #[cfg(feature = "semantic")]
        Commands::IndexAll { watch } => cmd_index_all(config, watch),
        Commands::Search { query, limit } => cmd_search(config, &query, limit, json),
        Commands::Callers {
            symbol,
            limit,
            min_confidence,
        } => cmd_callers(config, &symbol, limit, min_confidence, json),
        Commands::Impact {
            symbol,
            depth,
            min_confidence,
        } => cmd_impact(config, &symbol, depth, min_confidence, json),
        Commands::History { file, symbol } => cmd_history(config, &file, symbol.as_deref(), json),
        Commands::Context { file } => cmd_context(config, &file, json),
        Commands::Map { limit } => cmd_map(config, limit, json),
        Commands::Where { file } => cmd_where(config, &file, json),
        Commands::Stats => cmd_stats(config, json),
        Commands::Hook { action } => cmd_hook(config, action, json),
        Commands::Attach { host, local } => cmd_attach(&host, local),
        Commands::Mcp => cmd_mcp(config),
    }
}

const DEFAULT_MAX_FILES_NO_GIT: u64 = 50_000;

fn check_workspace_safety(workspace: &std::path::Path, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let home = std::env::var("HOME")
        .ok()
        .and_then(|h| std::fs::canonicalize(h).ok());
    let banned: Vec<std::path::PathBuf> = [home, Some(std::path::PathBuf::from("/"))]
        .into_iter()
        .flatten()
        .collect();
    for b in &banned {
        if workspace == b {
            anyhow::bail!(
                "refusing to index {} (too broad). Use --force to override, or --workspace to target a project directory.",
                workspace.display()
            );
        }
    }
    Ok(())
}

fn cmd_index(
    mut config: Config,
    json: bool,
    force: bool,
    max_files: Option<u64>,
    index_generated: bool,
) -> Result<()> {
    check_workspace_safety(&config.workspace, force)?;
    config.index.index_generated = index_generated;

    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let mut pipeline = IndexPipeline::new(config.clone());

    let has_git = config.workspace.join(".git").exists();
    let limit = match max_files {
        Some(0) => None,
        Some(n) => Some(n),
        None if force || has_git => None,
        None => Some(DEFAULT_MAX_FILES_NO_GIT),
    };
    if let Some(n) = limit {
        pipeline.set_max_files(n);
    }

    let stats = pipeline.run_structural(&repo)?;

    if json {
        println!("{}", serde_json::to_string(&stats)?);
    } else {
        eprintln!(
            "✓ {} files, {} chunks, {} symbols, {} refs in {}ms",
            stats.total_files,
            stats.total_chunks,
            stats.total_symbols,
            stats.refs,
            stats.duration_ms
        );
    }
    Ok(())
}

#[cfg(feature = "semantic")]
fn cmd_embed(config: Config) -> Result<()> {
    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config);

    let stats = pipeline.run_embedding(&repo, &embedder)?;

    eprintln!(
        "✓ embedded {} chunks, skipped {}, in {:.1}s",
        stats.embedded,
        stats.skipped,
        stats.duration_ms as f64 / 1000.0
    );

    Ok(())
}

#[cfg(feature = "semantic")]
fn cmd_index_all(config: Config, watch: bool) -> Result<()> {
    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config.clone());

    let stats = pipeline.run(&repo, &embedder)?;

    eprintln!(
        "✓ {} files, {} chunks, {} symbols in {:.1}s (embed: {:.1}s)",
        stats.total_files,
        stats.total_chunks,
        stats.total_symbols,
        stats.duration_ms as f64 / 1000.0,
        stats.embed_duration_ms as f64 / 1000.0
    );

    if watch {
        let watcher = code_abyss::watcher::FileWatcher::new(config);
        watcher.watch(&repo, Some(&embedder), &pipeline)?;
    }

    Ok(())
}

fn cmd_search(config: Config, query: &str, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    #[cfg(feature = "semantic")]
    let embedder = if repo.has_vectors()? {
        Embedder::load(&config.model).ok()
    } else {
        None
    };
    #[cfg(not(feature = "semantic"))]
    let embedder: Option<code_abyss::embedding::Embedder> = None;
    let engine = code_abyss::search::SearchEngine::new(&repo, embedder.as_ref());
    let results = engine.search(query, limit)?;

    if json {
        println!("{}", serde_json::to_string(&results)?);
    } else {
        if results.is_empty() {
            eprintln!("no results");
            return Ok(());
        }
        for (i, r) in results.iter().enumerate() {
            println!(
                "{}. {} (L{}-L{}) [{}] score={:.4}",
                i + 1,
                r.file_path,
                r.start_line + 1,
                r.end_line + 1,
                r.kind,
                r.score
            );
            let preview: Vec<&str> = r.content.lines().take(3).collect();
            for line in &preview {
                println!("   {line}");
            }
            if r.content.lines().count() > 3 {
                println!("   ...");
            }
            println!();
        }
    }
    Ok(())
}

fn cmd_callers(
    config: Config,
    symbol: &str,
    limit: usize,
    min_confidence: f64,
    json: bool,
) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    let callers = gq.find_callers(symbol, limit, min_confidence)?;

    if json {
        println!("{}", serde_json::to_string(&callers)?);
    } else {
        if callers.is_empty() {
            eprintln!("no callers found for '{symbol}'");
            return Ok(());
        }
        println!("callers of '{symbol}' ({} found):\n", callers.len());
        for (i, c) in callers.iter().enumerate() {
            let t = if c.is_test { " [test]" } else { "" };
            println!(
                "  {}. {}:{} → {}(){t}  ({:.0}%)",
                i + 1,
                c.file_path,
                c.line + 1,
                c.symbol,
                c.confidence * 100.0
            );
        }
    }
    Ok(())
}

fn cmd_impact(
    config: Config,
    symbol: &str,
    depth: u32,
    min_confidence: f64,
    json: bool,
) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    let result = gq.impact_analysis(symbol, depth, min_confidence)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "impact: {}  direct={}  transitive={}  tests={}  uncovered={}  risk={:.1}/10",
            result.target,
            result.direct_callers.len(),
            result.transitive_callers.len(),
            result.affected_tests.len(),
            result.uncovered_paths.len(),
            result.risk_score
        );
        for f in &result.risk_factors {
            println!("  ⚠ {f}");
        }
        for c in result.direct_callers.iter().take(10) {
            let t = if c.is_test { " [test]" } else { "" };
            println!("  {}:{} → {}(){t}", c.file_path, c.line + 1, c.symbol);
        }
    }
    Ok(())
}

fn cmd_history(config: Config, file: &str, symbol: Option<&str>, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let result =
        code_abyss::temporal::evolution::trace_evolution(&config.workspace, &repo, file, symbol)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "evolution: {}  changes={}  authors={}  churn={:.1}x",
            result.file_path, result.total_changes, result.unique_authors, result.churn_rate
        );
        for c in result.commits.iter().take(10) {
            println!("  {} {} {:<16} {}", c.hash, c.date, c.author, c.message);
        }
        for c in result.coupled_files.iter().take(5) {
            println!(
                "  ↔ {} ({}×, {:.0}%)",
                c.path,
                c.co_changes,
                c.coupling_score * 100.0
            );
        }
    }
    Ok(())
}

fn cmd_context(config: Config, file: &str, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let Some(output) = code_abyss::context::build_file_context(&repo, file)? else {
        eprintln!("file not found: {file}");
        return Ok(());
    };
    let file_path = output["file"].as_str().unwrap_or(file).to_string();
    let sym_callers = output["symbols_with_external_callers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let deps = output["dependencies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let hotspot = output.get("hotspot").filter(|h| !h.is_null()).cloned();
    let coupled = output["coupled_files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let symbols_defined = output["symbols_defined"].as_u64().unwrap_or(0);

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("=== {} ===\n", file_path);
        println!(
            "{} symbols defined, {} with external callers\n",
            symbols_defined,
            sym_callers.len()
        );

        for sc in &sym_callers {
            let sym = sc["symbol"].as_str().unwrap_or("");
            let callers = sc["external_callers"].as_array().unwrap();
            println!("  {}() ← {} callers", sym, callers.len());
            // Production callers are the pre-edit safety contract: list ALL of
            // them — silent truncation once made an agent miss a call site.
            // Test callers are capped, with the remainder counted explicitly.
            let (prod, test): (Vec<_>, Vec<_>) = callers
                .iter()
                .partition(|c| !c["is_test"].as_bool().unwrap_or(false));
            for c in &prod {
                println!(
                    "    {}:{} → {}()",
                    c["file"].as_str().unwrap_or(""),
                    c["line"],
                    c["caller"].as_str().unwrap_or("")
                );
            }
            for c in test.iter().take(3) {
                println!(
                    "    {}:{} → {}() [test]",
                    c["file"].as_str().unwrap_or(""),
                    c["line"],
                    c["caller"].as_str().unwrap_or("")
                );
            }
            if test.len() > 3 {
                println!("    … and {} more test callers", test.len() - 3);
            }
        }

        if !deps.is_empty() {
            println!("\n  depends on:");
            for d in &deps {
                println!(
                    "    → {} ({})",
                    d["name"].as_str().unwrap_or(""),
                    d["file"].as_str().unwrap_or("")
                );
            }
        }

        if let Some(h) = &hotspot {
            println!(
                "\n  hotspot: score={:.0}  changes={}  cc={:.0}",
                h["score"].as_f64().unwrap_or(0.0),
                h["changes_30d"].as_i64().unwrap_or(0),
                h["complexity"].as_f64().unwrap_or(0.0)
            );
        }

        if !coupled.is_empty() {
            println!("\n  coupled files:");
            for c in &coupled {
                println!(
                    "    ↔ {} ({}×, {})",
                    c["file"].as_str().unwrap_or(""),
                    c["co_changes"],
                    c["coupling"].as_str().unwrap_or("")
                );
            }
        }
    }
    Ok(())
}

fn cmd_map(config: Config, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let hotspots = code_abyss::temporal::hotspot::top_hotspots(&repo, limit)?;
    let coupled = code_abyss::temporal::coupling::top_coupled(&repo, limit)?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "hotspots": hotspots, "coupling": coupled
            }))?
        );
    } else {
        println!("═══ Hotspots ═══");
        for (i, h) in hotspots.iter().enumerate() {
            println!(
                "  {:2}. {:<55} score={:.0}  Δ{}  cc={:.0}  👤{}",
                i + 1,
                h.file_path,
                h.hotspot_score,
                h.change_count,
                h.complexity,
                h.unique_authors
            );
        }
        if !coupled.is_empty() {
            println!("\n═══ Coupling ═══");
            for (i, c) in coupled.iter().take(10).enumerate() {
                println!(
                    "  {:2}. {} ↔ {}  ({}×, {:.0}%)",
                    i + 1,
                    c.file_a,
                    c.file_b,
                    c.co_changes,
                    c.coupling_score * 100.0
                );
            }
        }
    }
    Ok(())
}

fn cmd_where(config: Config, file: &str, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let Some(view) = code_abyss::context::where_summary(&repo, file)? else {
        if json {
            println!("{}", serde_json::json!({"file": file, "found": false}));
        } else {
            eprintln!("not indexed: {file}");
        }
        return Ok(());
    };

    if json {
        println!("{}", serde_json::to_string(&view)?);
        return Ok(());
    }

    let layer = view["layer"].as_str().unwrap_or("unknown");
    let role = view["role"].as_str().unwrap_or("unknown");
    let module = view["module_label"].as_str().unwrap_or("—");
    let conf = view["layer_conf"].as_f64().unwrap_or(0.0);
    let centrality = view["centrality"].as_f64().unwrap_or(0.0);
    let in_deg = view["in_degree"].as_u64().unwrap_or(0);
    let out_deg = view["out_degree"].as_u64().unwrap_or(0);
    let depth = view["depth_from_entry"]
        .as_u64()
        .map(|d| d.to_string())
        .unwrap_or_else(|| String::from("—"));
    let path = view["file"].as_str().unwrap_or(file);

    println!("where: {path}");
    println!("  layer={layer}  module={module}  role={role}  conf={conf:.2}");
    println!("  depth_from_entry={depth}  centrality={centrality:.2}  in={in_deg} out={out_deg}");

    // Compact one-line signal trace — useful for "why is this file classified
    // as X?" without dumping the full JSON.
    let signals = &view["signals"];
    if !signals.is_null() {
        let dir_hints = signals["dir"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|h| {
                        format!(
                            "{}×{:.1}",
                            h["layer"].as_str().unwrap_or("?"),
                            h["weight"].as_f64().unwrap_or(0.0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let name_hints = signals["name"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|h| {
                        format!(
                            "{}×{:.1}",
                            h["layer"].as_str().unwrap_or("?"),
                            h["weight"].as_f64().unwrap_or(0.0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let entry = signals["entry"].as_bool().unwrap_or(false);
        println!("  signals: dir=[{dir_hints}], name=[{name_hints}], entry={entry}");
    }
    if let Some(note) = view.get("note").and_then(|v| v.as_str()) {
        println!("  note: {note}");
    }
    Ok(())
}

fn cmd_stats(config: Config, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let s = serde_json::json!({
        "files": repo.file_count()?,
        "chunks": repo.chunk_count()?,
        "symbols": repo.symbol_count()?,
        "refs": repo.ref_count()?,
        "workspace": config.workspace.display().to_string(),
    });

    if json {
        println!("{}", serde_json::to_string(&s)?);
    } else {
        println!(
            "abyss: {} files, {} chunks, {} symbols, {} refs",
            s["files"], s["chunks"], s["symbols"], s["refs"]
        );
    }
    Ok(())
}

const HOOK_LANGS: [&str; 17] = [
    "go", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "java", "c", "h", "cpp", "cc",
    "cxx", "hpp",
];

fn cmd_hook(config: Config, action: HookAction, json: bool) -> Result<()> {
    // Hooks must never block the agent: every early-out is a silent success.
    match action {
        HookAction::PreEdit => hook_pre_edit(config, json),
        HookAction::PostEdit => hook_post_edit(config),
    }
}

fn read_stdin_json() -> Option<serde_json::Value> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).ok()?;
    serde_json::from_str(buf.trim()).ok()
}

fn hook_pre_edit(config: Config, json: bool) -> Result<()> {
    let start = std::time::Instant::now();
    let Some(payload) = read_stdin_json() else {
        return Ok(());
    };
    let Some(raw_path) = code_abyss::context::extract_file_path(&payload) else {
        return Ok(());
    };
    let ext = raw_path.rsplit('.').next().unwrap_or("");
    if !HOOK_LANGS.contains(&ext) {
        return Ok(());
    }
    // Opt-in: only fire when the project has an index.
    if !config.db_path.exists() {
        return Ok(());
    }

    // Read-only path: hooks must never block the agent. A full structural
    // refresh on every PreToolUse blocks the agent on a repo scan before
    // every edit — the #1 reason ambient delivery was broken. Index updates
    // run from hook_post_edit instead; pre_edit only queries.
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;

    let rel = std::path::Path::new(&raw_path)
        .strip_prefix(&config.workspace)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| raw_path.replace('\\', "/"));

    let Some(mut ctx) = code_abyss::context::build_file_context(&repo, &rel)? else {
        return Ok(());
    };

    // Best-effort enrichment for the card. All failures are silent — the
    // hook must never block the agent on a missing optional metric.
    enrich_ctx_for_card(&repo, &rel, &mut ctx);

    if json {
        println!("{}", serde_json::to_string(&ctx)?);
    }

    let staleness_ms = start.elapsed().as_millis();
    let card = code_abyss::context::render_card(&ctx, &rel, staleness_ms);
    eprintln!("{card}");

    Ok(())
}

/// Add `siblings`, `epoch`, and `last_touched_days` to the context payload
/// so `render_card` has the data it needs. Every query is best-effort.
fn enrich_ctx_for_card(repo: &Repository, rel: &str, ctx: &mut serde_json::Value) {
    let conn = repo.conn();
    let obj = match ctx.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // ---- siblings: other files in the same directory ----
    let dir = match rel.rsplit_once('/') {
        Some((d, _)) => format!("{d}/"),
        None => String::new(),
    };
    let fname = rel.rsplit('/').next().unwrap_or(rel);
    let siblings: Vec<serde_json::Value> = (|| -> rusqlite::Result<Vec<serde_json::Value>> {
        let mut stmt = conn.prepare(
            "SELECT path FROM files WHERE dir = ?1 AND path != ?2 ORDER BY path LIMIT 12",
        )?;
        let rows = stmt.query_map([&dir, &rel.to_string()], |r| r.get::<_, String>(0))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .map(|p| serde_json::Value::String(p.rsplit('/').next().unwrap_or(&p).to_string()))
            .collect())
    })()
    .unwrap_or_default();
    if !siblings.is_empty() {
        obj.insert("siblings".into(), serde_json::Value::Array(siblings));
    }
    let _ = fname;

    // ---- epoch: latest known commit ts (workspace-wide, fast aggregate) ----
    if let Ok(epoch) =
        conn.query_row::<i64, _, _>("SELECT COALESCE(MAX(ts), 0) FROM commits", [], |r| r.get(0))
    {
        obj.insert("epoch".into(), serde_json::json!(epoch));
    }

    // ---- last_touched_days: based on file_metrics.last_changed_ts ----
    if let Ok(last_ts) = conn.query_row::<i64, _, _>(
        "SELECT COALESCE(fm.last_changed_ts, 0) FROM file_metrics fm
         JOIN files f ON f.id = fm.file_id WHERE f.path = ?1",
        [rel],
        |r| r.get(0),
    ) && last_ts > 0
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let days = ((now - last_ts) / 86_400).max(0);
        obj.insert("last_touched_days".into(), serde_json::json!(days));
    }
}

fn hook_post_edit(config: Config) -> Result<()> {
    if !config.db_path.exists() {
        return Ok(());
    }
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config);
    let _ = pipeline.run_structural(&repo);
    Ok(())
}

fn cmd_attach(host: &str, local: bool) -> Result<()> {
    match host {
        "claude" => code_abyss::attach::claude::install(local),
        other => anyhow::bail!("unknown host: {other}; supported: claude"),
    }
}

fn cmd_mcp(config: Config) -> Result<()> {
    check_workspace_safety(&config.workspace, false)?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        info!("starting MCP server");

        let repo = Repository::open(&config.db_path, config.model.dimensions)?;
        let pipeline = IndexPipeline::new(config.clone());

        // Fast structural index on startup
        let stats = pipeline.run_structural(&repo)?;
        info!(
            "index ready: {} files, {} chunks",
            stats.total_files, stats.total_chunks
        );

        // Load embedding model (semantic builds only)
        #[cfg(feature = "semantic")]
        let embedder = match Embedder::load(&config.model) {
            Ok(e) => {
                info!("embedding model loaded");
                Some(e)
            }
            Err(e) => {
                info!("embedding model unavailable: {e} (semantic search disabled)");
                None
            }
        };
        #[cfg(not(feature = "semantic"))]
        let embedder: Option<code_abyss::embedding::Embedder> = None;

        let repo_arc = std::sync::Arc::new(std::sync::Mutex::new(repo));
        let embedder_arc = std::sync::Arc::new(embedder);
        let pipeline_arc = std::sync::Arc::new(pipeline);

        // Background embedding thread
        if embedder_arc.is_some() {
            let bg_repo_path = config.db_path.clone();
            let bg_config = config.clone();
            let bg_embedder = embedder_arc.clone();
            let bg_pipeline = pipeline_arc.clone();

            std::thread::spawn(move || {
                info!("background embedding started");
                let bg_repo = match Repository::open(&bg_repo_path, bg_config.model.dimensions) {
                    Ok(r) => r,
                    Err(e) => {
                        info!("background embedding failed to open db: {e}");
                        return;
                    }
                };
                if let Some(ref emb) = *bg_embedder {
                    match bg_pipeline.run_embedding(&bg_repo, emb) {
                        Ok(stats) => info!(
                            "background embedding done: {} embedded, {} skipped in {:.1}s",
                            stats.embedded,
                            stats.skipped,
                            stats.duration_ms as f64 / 1000.0
                        ),
                        Err(e) => info!("background embedding error: {e}"),
                    }
                }
            });
        }

        let server = McpServer {
            repo: repo_arc,
            embedder: embedder_arc,
            pipeline: pipeline_arc,
            config,
        };

        let transport = rmcp::transport::io::stdio();
        let service = server.serve(transport).await?;
        info!("MCP server running (stdio)");
        service.waiting().await?;

        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
