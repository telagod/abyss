use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, error};
use tracing_subscriber::EnvFilter;

use code_abyss::config::Config;
use code_abyss::embedding::Embedder;
use code_abyss::indexer::IndexPipeline;
use code_abyss::mcp::McpServer;
use rmcp::ServiceExt;
use code_abyss::storage::Repository;

#[derive(Parser)]
#[command(name = "abyss", version, about = "Code relationship graph and temporal intelligence")]
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
    Index,
    /// Generate embeddings for semantic search. Slow, run after `index`.
    Embed,
    /// Full index + embed in one shot
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
    },
    /// Analyze blast radius of changing a symbol
    Impact {
        symbol: String,
        #[arg(short, long, default_value = "3")]
        depth: u32,
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
    /// Show index statistics
    Stats,
    /// Run as MCP server (stdio transport)
    Mcp,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let workspace = std::fs::canonicalize(&cli.workspace)?;
    let mut config = Config::new(&workspace);

    if let Some(db) = cli.db { config.db_path = db; }
    if let Some(model) = cli.model { config.model.model_id = model; }

    let json = cli.json;

    match cli.command {
        Commands::Index => cmd_index(config, json),
        Commands::Embed => cmd_embed(config),
        Commands::IndexAll { watch } => cmd_index_all(config, watch),
        Commands::Search { query, limit } => cmd_search(config, &query, limit, json),
        Commands::Callers { symbol, limit } => cmd_callers(config, &symbol, limit, json),
        Commands::Impact { symbol, depth } => cmd_impact(config, &symbol, depth, json),
        Commands::History { file, symbol } => cmd_history(config, &file, symbol.as_deref(), json),
        Commands::Context { file } => cmd_context(config, &file, json),
        Commands::Map { limit } => cmd_map(config, limit, json),
        Commands::Stats => cmd_stats(config, json),
        Commands::Mcp => cmd_mcp(config),
    }
}

fn cmd_index(config: Config, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config.clone());
    let stats = pipeline.run_structural(&repo)?;

    if json {
        println!("{}", serde_json::to_string(&stats)?);
    } else {
        eprintln!("✓ {} files, {} chunks, {} symbols, {} refs in {}ms",
            stats.total_files, stats.total_chunks, stats.total_symbols, stats.refs, stats.duration_ms);
    }
    Ok(())
}

fn cmd_embed(config: Config) -> Result<()> {
    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config);

    let stats = pipeline.run_embedding(&repo, &embedder)?;

    eprintln!(
        "✓ embedded {} chunks, skipped {}, in {:.1}s",
        stats.embedded, stats.skipped, stats.duration_ms as f64 / 1000.0
    );

    Ok(())
}

fn cmd_index_all(config: Config, watch: bool) -> Result<()> {
    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config.clone());

    let stats = pipeline.run(&repo, &embedder)?;

    eprintln!(
        "✓ {} files, {} chunks, {} symbols in {:.1}s (embed: {:.1}s)",
        stats.total_files, stats.total_chunks, stats.total_symbols,
        stats.duration_ms as f64 / 1000.0,
        stats.embed_duration_ms as f64 / 1000.0
    );

    if watch {
        let watcher = code_abyss::watcher::FileWatcher::new(config);
        watcher.watch(&repo, &embedder, &pipeline)?;
    }

    Ok(())
}

fn cmd_search(config: Config, query: &str, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let has_vectors = repo.has_vectors()?;
    let embedder = if has_vectors { Embedder::load(&config.model).ok() } else { None };
    let engine = code_abyss::search::SearchEngine::new(&repo, embedder.as_ref());
    let results = engine.search(query, limit)?;

    if json {
        println!("{}", serde_json::to_string(&results)?);
    } else {
        if results.is_empty() { eprintln!("no results"); return Ok(()); }
        for (i, r) in results.iter().enumerate() {
            println!("{}. {} (L{}-L{}) [{}] score={:.4}",
                i + 1, r.file_path, r.start_line + 1, r.end_line + 1, r.kind, r.score);
            let preview: Vec<&str> = r.content.lines().take(3).collect();
            for line in &preview { println!("   {line}"); }
            if r.content.lines().count() > 3 { println!("   ..."); }
            println!();
        }
    }
    Ok(())
}

fn cmd_callers(config: Config, symbol: &str, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    let callers = gq.find_callers(symbol, limit)?;

    if json {
        println!("{}", serde_json::to_string(&callers)?);
    } else {
        if callers.is_empty() { eprintln!("no callers found for '{symbol}'"); return Ok(()); }
        println!("callers of '{symbol}' ({} found):\n", callers.len());
        for (i, c) in callers.iter().enumerate() {
            let t = if c.is_test { " [test]" } else { "" };
            println!("  {}. {}:{} → {}(){t}  ({:.0}%)",
                i + 1, c.file_path, c.line + 1, c.symbol, c.confidence * 100.0);
        }
    }
    Ok(())
}

fn cmd_impact(config: Config, symbol: &str, depth: u32, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    let result = gq.impact_analysis(symbol, depth)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("impact: {}  direct={}  transitive={}  tests={}  uncovered={}  risk={:.1}/10",
            result.target, result.direct_callers.len(), result.transitive_callers.len(),
            result.affected_tests.len(), result.uncovered_paths.len(), result.risk_score);
        for f in &result.risk_factors { println!("  ⚠ {f}"); }
        for c in result.direct_callers.iter().take(10) {
            let t = if c.is_test { " [test]" } else { "" };
            println!("  {}:{} → {}(){t}", c.file_path, c.line + 1, c.symbol);
        }
    }
    Ok(())
}

fn cmd_history(config: Config, file: &str, symbol: Option<&str>, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let result = code_abyss::temporal::evolution::trace_evolution(&config.workspace, &repo, file, symbol)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("evolution: {}  changes={}  authors={}  churn={:.1}x",
            result.file_path, result.total_changes, result.unique_authors, result.churn_rate);
        for c in result.commits.iter().take(10) {
            println!("  {} {} {:<16} {}", c.hash, c.date, c.author, c.message);
        }
        for c in result.coupled_files.iter().take(5) {
            println!("  ↔ {} ({}×, {:.0}%)", c.path, c.co_changes, c.coupling_score * 100.0);
        }
    }
    Ok(())
}

fn cmd_context(config: Config, file: &str, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let conn = repo.conn();

    // Find file
    let file_id: i64 = match conn.query_row(
        "SELECT id FROM files WHERE path = ?1 OR path LIKE ?2",
        rusqlite::params![file, format!("%{file}")],
        |r| r.get(0),
    ) {
        Ok(id) => id,
        Err(_) => { eprintln!("file not found: {file}"); return Ok(()); }
    };

    let file_path: String = repo.get_file_path(file_id)?.unwrap_or_default();

    // Get all symbols defined in this file
    let symbols = repo.find_symbols_in_file(file_id)?;

    // For each symbol, find external callers
    let mut sym_callers: Vec<serde_json::Value> = Vec::new();
    for sym in &symbols {
        let callers = repo.find_callers_of(&sym.name, Some(file_id), 10)?;
        let external: Vec<_> = callers.iter()
            .filter(|c| c.source_file_id != file_id)
            .collect();
        if !external.is_empty() {
            sym_callers.push(serde_json::json!({
                "symbol": sym.name,
                "kind": sym.kind,
                "line": sym.line + 1,
                "external_callers": external.iter().map(|c| {
                    serde_json::json!({
                        "file": &c.source_file_path,
                        "line": c.source_line + 1,
                        "caller": &c.source_symbol,
                        "is_test": repo.is_test_file(c.source_file_id).unwrap_or(false),
                    })
                }).collect::<Vec<_>>(),
            }));
        }
    }

    // Get outgoing refs (what this file depends on)
    let mut deps_stmt = conn.prepare(
        "SELECT DISTINCT r.target_name, f.path, r.kind
         FROM refs r LEFT JOIN files f ON r.target_file_id = f.id
         WHERE r.source_file_id = ?1 AND r.kind IN ('call','type_ref')
         AND r.target_file_id IS NOT NULL AND r.target_file_id != ?1
         LIMIT 20")?;
    let deps: Vec<serde_json::Value> = deps_stmt.query_map([file_id], |row| {
        Ok(serde_json::json!({
            "name": row.get::<_, String>(0)?,
            "file": row.get::<_, String>(1)?,
            "kind": row.get::<_, String>(2)?,
        }))
    })?.filter_map(|r| r.ok()).collect();

    // Hotspot info
    let hotspot: Option<(f64, i64, f64)> = conn.query_row(
        "SELECT hotspot_score, change_count_30d, cyclomatic FROM file_metrics WHERE file_id = ?1",
        [file_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).ok();

    // Coupled files
    let mut coupling_stmt = conn.prepare(
        "SELECT file_b, co_changes, coupling_score FROM change_coupling WHERE file_a = ?1
         UNION SELECT file_a, co_changes, coupling_score FROM change_coupling WHERE file_b = ?1
         ORDER BY coupling_score DESC LIMIT 5")?;
    let coupled: Vec<serde_json::Value> = coupling_stmt.query_map([&file_path], |row| {
        Ok(serde_json::json!({
            "file": row.get::<_, String>(0)?,
            "co_changes": row.get::<_, i64>(1)?,
            "coupling": format!("{:.0}%", row.get::<_, f64>(2)? * 100.0),
        }))
    })?.filter_map(|r| r.ok()).collect();

    let output = serde_json::json!({
        "file": file_path,
        "symbols_defined": symbols.len(),
        "symbols_with_external_callers": sym_callers,
        "dependencies": deps,
        "hotspot": hotspot.map(|(score, changes, cc)| serde_json::json!({
            "score": score, "changes_30d": changes, "complexity": cc
        })),
        "coupled_files": coupled,
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("=== {} ===\n", file_path);
        println!("{} symbols defined, {} with external callers\n",
            symbols.len(), sym_callers.len());

        for sc in &sym_callers {
            let sym = sc["symbol"].as_str().unwrap_or("");
            let callers = sc["external_callers"].as_array().unwrap();
            println!("  {}() ← {} callers", sym, callers.len());
            for c in callers.iter().take(5) {
                let t = if c["is_test"].as_bool().unwrap_or(false) { " [test]" } else { "" };
                println!("    {}:{} → {}(){t}",
                    c["file"].as_str().unwrap_or(""), c["line"], c["caller"].as_str().unwrap_or(""));
            }
        }

        if !deps.is_empty() {
            println!("\n  depends on:");
            for d in &deps {
                println!("    → {} ({})", d["name"].as_str().unwrap_or(""), d["file"].as_str().unwrap_or(""));
            }
        }

        if let Some(h) = &hotspot {
            println!("\n  hotspot: score={:.0}  changes={}  cc={:.0}", h.0, h.1, h.2);
        }

        if !coupled.is_empty() {
            println!("\n  coupled files:");
            for c in &coupled {
                println!("    ↔ {} ({}×, {})", c["file"].as_str().unwrap_or(""),
                    c["co_changes"], c["coupling"].as_str().unwrap_or(""));
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
        println!("{}", serde_json::to_string(&serde_json::json!({
            "hotspots": hotspots, "coupling": coupled
        }))?);
    } else {
        println!("═══ Hotspots ═══");
        for (i, h) in hotspots.iter().enumerate() {
            println!("  {:2}. {:<55} score={:.0}  Δ{}  cc={:.0}  👤{}",
                i + 1, h.file_path, h.hotspot_score, h.change_count, h.complexity, h.unique_authors);
        }
        if !coupled.is_empty() {
            println!("\n═══ Coupling ═══");
            for (i, c) in coupled.iter().take(10).enumerate() {
                println!("  {:2}. {} ↔ {}  ({}×, {:.0}%)",
                    i + 1, c.file_a, c.file_b, c.co_changes, c.coupling_score * 100.0);
            }
        }
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
        println!("abyss: {} files, {} chunks, {} symbols, {} refs",
            s["files"], s["chunks"], s["symbols"], s["refs"]);
    }
    Ok(())
}

fn cmd_mcp(config: Config) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        info!("starting MCP server");

        let repo = Repository::open(&config.db_path, config.model.dimensions)?;
        let pipeline = IndexPipeline::new(config.clone());

        // Fast structural index on startup
        let stats = pipeline.run_structural(&repo)?;
        info!("index ready: {} files, {} chunks", stats.total_files, stats.total_chunks);

        // Load embedding model
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
                    Err(e) => { info!("background embedding failed to open db: {e}"); return; }
                };
                if let Some(ref emb) = *bg_embedder {
                    match bg_pipeline.run_embedding(&bg_repo, emb) {
                        Ok(stats) => info!(
                            "background embedding done: {} embedded, {} skipped in {:.1}s",
                            stats.embedded, stats.skipped, stats.duration_ms as f64 / 1000.0
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
