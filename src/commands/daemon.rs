use anyhow::{Context, Result};

use crate::config::Config;
use crate::indexer::IndexPipeline;
use crate::storage::Repository;

pub fn cmd_daemon(config: Config, action: super::DaemonCmd) -> Result<()> {
    use crate::daemon::{DaemonAction, run};
    let mapped = match action {
        super::DaemonCmd::Start { foreground, detach } => {
            DaemonAction::Start { foreground, detach }
        }
        super::DaemonCmd::Stop => DaemonAction::Stop,
        super::DaemonCmd::Status => DaemonAction::Status,
        super::DaemonCmd::Logs { tail, follow } => DaemonAction::Logs { tail, follow },
    };
    run(config, mapped)
}

/// V2 daemon path: connect to `<workspace>/.code-abyss/daemon.sock`, send
/// the `{"cmd":"mcp"}` switch verb, then pipe the agent's stdio
/// bidirectionally through the socket until either side closes.
///
/// This is opt-in for two reasons:
/// 1. Standalone `abyss mcp` keeps working unchanged so existing MCP
///    configs (Claude Desktop, Cursor, etc.) need no migration.
/// 2. When no daemon is running we error out with an actionable message
///    instead of silently falling back — agents that asked for daemon
///    sharing usually mean it (e.g. wanted multi-reader semantics or
///    avoided re-indexing on every spawn).
#[cfg(unix)]
pub fn cmd_mcp_via_daemon(config: Config) -> Result<()> {
    use crate::daemon::DaemonPaths;
    let paths = DaemonPaths::from_config(&config);
    if !paths.socket.exists() {
        anyhow::bail!(
            "abyss mcp --via-daemon: no daemon found at {}; either start one with `abyss daemon start` or drop --via-daemon for standalone mode",
            paths.socket.display()
        );
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    rt.block_on(async move {
        use tokio::io::AsyncWriteExt;
        let std_stream = std::os::unix::net::UnixStream::connect(&paths.socket)
            .with_context(|| format!("connect {}", paths.socket.display()))?;
        std_stream.set_nonblocking(true)?;
        let mut stream = tokio::net::UnixStream::from_std(std_stream)?;

        // Switch the connection into MCP mode. Newline-terminated — the
        // daemon's request reader looks at one line then takes the rest
        // of the fd over for rmcp.
        stream.write_all(b"{\"cmd\":\"mcp\"}\n").await?;
        stream.flush().await?;

        let (sock_read, sock_write) = stream.into_split();
        let mut stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();

        // Two-way splice: stdin → socket, socket → stdout. Either side
        // closing tears down the other so we don't leak fds when the
        // agent's MCP client disconnects.
        let up = async move {
            let mut s = sock_write;
            let _ = tokio::io::copy(&mut stdin, &mut s).await;
            // Half-close upstream so the daemon sees EOF and stops its
            // rmcp loop. Best-effort: `shutdown` can fail if the peer
            // already went away.
            let _ = s.shutdown().await;
        };
        let down = async move {
            let mut r = sock_read;
            let _ = tokio::io::copy(&mut r, &mut stdout).await;
            let _ = stdout.shutdown().await;
        };

        tokio::select! {
            _ = up => {}
            _ = down => {}
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

#[cfg(not(unix))]
pub fn cmd_mcp_via_daemon(_config: Config) -> Result<()> {
    anyhow::bail!("`abyss mcp --via-daemon` is Unix-only; use `abyss mcp` for standalone mode");
}

pub fn cmd_mcp(config: Config) -> Result<()> {
    use crate::mcp::McpServer;
    use rmcp::ServiceExt;
    use tracing::info;

    super::index::check_workspace_safety(&config.workspace, false)?;
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
        let embedder = match crate::embedding::Embedder::load(&config.model) {
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
        let embedder: Option<crate::embedding::Embedder> = None;

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

pub fn cmd_watch(config: Config, debounce_ms: u64) -> Result<()> {
    // Refuse before any work: the watch loop is long-running, no point opening
    // a fresh repo against a missing index.
    if !config.db_path.exists() {
        anyhow::bail!(
            "no index found at {} — run `abyss index` first",
            config.db_path.display()
        );
    }

    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config.clone());

    // Semantic builds: try to load the embedder so vectors stay fresh on save.
    // Slim builds: None — structural reindex only (matches `abyss index`).
    #[cfg(feature = "semantic")]
    let embedder: Option<crate::embedding::Embedder> =
        match crate::embedding::Embedder::load(&config.model) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("[abyss watch] embedder unavailable ({e}) — structural only");
                None
            }
        };
    #[cfg(not(feature = "semantic"))]
    let embedder: Option<crate::embedding::Embedder> = None;

    let debounce = std::time::Duration::from_millis(debounce_ms);
    let watcher = crate::watcher::FileWatcher::new(config.clone()).with_debounce(debounce);

    eprintln!(
        "abyss watching {} (debounce {}ms) — Ctrl-C to stop",
        config.workspace.display(),
        debounce_ms
    );
    watcher.watch(&repo, embedder.as_ref(), &pipeline)?;
    Ok(())
}
