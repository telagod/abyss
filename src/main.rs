use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
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

    // No `short` here: `-d` is claimed by `impact --depth`. Globals conflict
    // with subcommand-local shorts when clap_complete walks the full command
    // tree, so we keep `--db` long-only.
    #[arg(long, global = true)]
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
        /// Restrict the index pass to files git reports as changed between
        /// `<ref>` and HEAD. Added/Modified/Renamed paths are reindexed;
        /// Deleted paths are dropped from the index. Skips the workspace
        /// walker entirely — orders of magnitude faster on large repos.
        ///
        /// Falls back to a full walk (with a stderr warning) when the
        /// workspace is not a git repo. Example: `abyss index --since HEAD~5`
        /// or `abyss index --since origin/main` from a feature branch.
        #[arg(long, value_name = "REF")]
        since: Option<String>,
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
    /// Find all callers of a symbol.
    ///
    /// Default returns ALL dependent kinds — call expressions, field
    /// accesses, type-position uses (annotations, generics, extends), AND
    /// inheritance edges. This matches what most agents mean by "who
    /// depends on this".
    ///
    /// Use one of the restricting flags to narrow the view:
    ///   * `--calls-only` — only direct call expressions (kind=`call`)
    ///   * `--types-only` — only type-position uses (kind=`type_ref`)
    ///   * `--inherits-only` — only inheritance (kind=`inherit`)
    ///   * `--all-deps` — explicit alias for the default (no-op; useful
    ///     for self-documenting scripts and pair-programming flows where
    ///     "I want everything" needs to be said out loud)
    Callers {
        symbol: String,
        /// Max results (default: 20). Use `0` for unlimited (50_000 internal
        /// cap so a hot framework primitive can't OOM the process). When
        /// the result is capped, a "showing N of M" footer tells you to
        /// rerun with `--limit 0` or a larger N.
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Hide references resolved below this confidence (0 shows everything)
        #[arg(long, default_value = "0.7")]
        min_confidence: f64,
        /// Include callers from test files (default: hide them so agents see prod call sites first)
        #[arg(long)]
        include_tests: bool,
        /// Restrict to direct call expressions (kind=`call`, plus
        /// `field_access` for the method-style invocations the resolver
        /// emits as method calls). Drops type-position users and
        /// inheritance edges. Mutually exclusive with `--types-only` /
        /// `--inherits-only` / `--all-deps`.
        #[arg(long, conflicts_with_all = ["types_only", "inherits_only", "all_deps"])]
        calls_only: bool,
        /// Restrict to type-position uses (kind=`type_ref` — annotations,
        /// generics, extends/implements clauses). Drops invocations and
        /// inheritance edges. Mutually exclusive with `--calls-only` /
        /// `--inherits-only` / `--all-deps`.
        #[arg(long, conflicts_with_all = ["calls_only", "inherits_only", "all_deps"])]
        types_only: bool,
        /// Restrict to inheritance edges (kind=`inherit` — every subclass
        /// of a base class). Drops invocations and type-position users.
        /// Mutually exclusive with `--calls-only` / `--types-only` /
        /// `--all-deps`. Surfaces every Django Model subclass when run
        /// against `Model`.
        #[arg(long, conflicts_with_all = ["calls_only", "types_only", "all_deps"])]
        inherits_only: bool,
        /// Explicit alias for the default behaviour: returns ALL dependent
        /// kinds (calls, field access, type uses, inheritance). Same set
        /// as passing no flag — provided as self-documenting sugar for
        /// scripts that want to say out loud "I want everything". Mutually
        /// exclusive with the restricting flags.
        #[arg(long, conflicts_with_all = ["calls_only", "types_only", "inherits_only"])]
        all_deps: bool,
    },
    /// Analyze blast radius of changing a symbol.
    ///
    /// Default includes ALL dependent kinds (calls + field access + type
    /// uses + inheritance) so the count matches `abyss callers` — a
    /// type-position user or subclass IS affected when you change the
    /// symbol's API. Use `--calls-only` to recover the legacy "who invokes
    /// this function" behaviour.
    Impact {
        symbol: String,
        #[arg(short, long, default_value = "3")]
        depth: u32,
        /// Exclude references resolved below this confidence (0 includes everything)
        #[arg(long, default_value = "0.7")]
        min_confidence: f64,
        /// Restrict to invocation edges (`call` + `field_access`). Legacy
        /// function-only blast radius — drops type-position users and
        /// inheritors. The default behaviour (no flag) matches what
        /// `abyss callers` reports.
        #[arg(long)]
        calls_only: bool,
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
    /// Run as MCP server (stdio transport).
    ///
    /// With `--via-daemon`, instead of running a standalone server this
    /// process connects to a running `abyss daemon` over its Unix socket
    /// and tunnels the agent's stdio MCP traffic through that connection.
    /// Multiple MCP clients sharing one daemon avoids per-client SQLite
    /// open / index-load latency. If no daemon is running, the flag errors
    /// out — drop `--via-daemon` to fall back to standalone mode.
    Mcp {
        /// Connect to a running `abyss daemon` instead of starting a
        /// standalone MCP server in this process.
        #[arg(long)]
        via_daemon: bool,
    },
    /// Foreground daemon-lite: watch the workspace and incrementally
    /// reindex on file save. Equivalent to `abyss daemon start --foreground`.
    Watch {
        /// Debounce window in milliseconds (default 150ms — tier-A target)
        #[arg(long, default_value_t = code_abyss::watcher::DEFAULT_DEBOUNCE_MS)]
        debounce_ms: u64,
    },
    /// Background daemon (Unix-only): pidfile-locked, Unix-socket fronted.
    /// `start [--foreground]` / `stop` / `status`.
    Daemon {
        #[command(subcommand)]
        action: DaemonCmd,
    },
    /// Print a shell-completion script to stdout (`eval`-able). Supports
    /// bash, zsh, fish, powershell, and elvish.
    ///
    /// Examples:
    ///   abyss completion bash | sudo tee /etc/bash_completion.d/abyss
    ///   abyss completion zsh  > ~/.config/zsh/_abyss
    ///   abyss completion fish > ~/.config/fish/completions/abyss.fish
    Completion {
        /// Target shell. One of: bash, zsh, fish, powershell, elvish.
        shell: clap_complete::Shell,
    },
    /// Inspect the effective abyss config: workspace paths, schema
    /// version, arch.toml overrides, dictionary rule counts, per-layer
    /// fact counts on the current index, and daemon liveness.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Clean `.code-abyss/` surfaces. Default scope removes the index DB
    /// only (keeps `arch.toml` user overrides). Scope flags compose:
    ///
    ///   * `--all` — wipe the entire `.code-abyss/` directory (DB + arch.toml +
    ///     daemon files). Use when you want a true greenfield reset.
    ///   * `--daemon` — only remove daemon.pid / daemon.sock / daemon.log.
    ///   * `--dry-run` — print "would remove: ..." for each target, mutate
    ///     nothing. Compose with any scope flag.
    ///
    /// Refuses to run while a daemon is alive against this workspace —
    /// stop it first with `abyss daemon stop` so the running process
    /// doesn't recreate the socket / pidfile mid-reset.
    Reset {
        /// Remove the entire `.code-abyss/` directory (incl. arch.toml).
        #[arg(long, conflicts_with = "daemon")]
        all: bool,
        /// Only remove daemon.pid / daemon.sock / daemon.log; preserve the DB.
        #[arg(long)]
        daemon: bool,
        /// Print what would be removed without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print effective config + state. TOML by default; `--json` for
    /// machine consumption. Read-only — never mutates state.
    Show,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Acquire pidfile lock, bind socket, start watching. Use `--detach` to
    /// double-fork into the background; `--foreground` keeps the daemon
    /// attached to the controlling terminal (overrides `--detach`).
    Start {
        /// Stay attached to the controlling terminal (don't redirect logs).
        #[arg(long)]
        foreground: bool,
        /// Detach via double-fork + setsid. The shell returns once the
        /// child has claimed the pidfile (≤500ms).
        #[arg(long)]
        detach: bool,
    },
    /// SIGTERM the recorded pid; wait up to 5s for cleanup.
    Stop,
    /// Print pid, uptime, last reindex, socket path. Exit 1 if not running.
    Status,
    /// Tail the daemon log (`.code-abyss/daemon.log`). Goes through the
    /// running daemon's socket when available, otherwise reads the file
    /// directly.
    Logs {
        /// How many trailing lines to print. Default 50.
        #[arg(long, default_value_t = 50)]
        tail: usize,
        /// Stream new lines as they're appended (think `tail -f`). After
        /// printing the initial tail, the CLI keeps polling the log file
        /// directly — no socket round-trip per line. Stop with Ctrl-C.
        #[arg(long, short = 'f')]
        follow: bool,
    },
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
            since,
        } => cmd_index(config, json, force, max_files, index_generated, since),
        #[cfg(feature = "semantic")]
        Commands::Embed => cmd_embed(config),
        #[cfg(feature = "semantic")]
        Commands::IndexAll { watch } => cmd_index_all(config, watch),
        Commands::Search { query, limit } => cmd_search(config, &query, limit, json),
        Commands::Callers {
            symbol,
            limit,
            min_confidence,
            include_tests,
            calls_only,
            types_only,
            inherits_only,
            all_deps,
        } => cmd_callers(
            config,
            &symbol,
            limit,
            min_confidence,
            include_tests,
            calls_only,
            types_only,
            inherits_only,
            all_deps,
            json,
        ),
        Commands::Impact {
            symbol,
            depth,
            min_confidence,
            calls_only,
        } => cmd_impact(config, &symbol, depth, min_confidence, calls_only, json),
        Commands::History { file, symbol } => cmd_history(config, &file, symbol.as_deref(), json),
        Commands::Context { file } => cmd_context(config, &file, json),
        Commands::Map { limit } => cmd_map(config, limit, json),
        Commands::Where { file } => cmd_where(config, &file, json),
        Commands::Stats => cmd_stats(config, json),
        Commands::Hook { action } => cmd_hook(config, action, json),
        Commands::Attach { host, local } => cmd_attach(&host, local),
        Commands::Mcp { via_daemon } => {
            if via_daemon {
                cmd_mcp_via_daemon(config)
            } else {
                cmd_mcp(config)
            }
        }
        Commands::Watch { debounce_ms } => cmd_watch(config, debounce_ms),
        Commands::Daemon { action } => cmd_daemon(config, action),
        Commands::Completion { shell } => cmd_completion(shell),
        Commands::Config { action } => cmd_config(config, action, json),
        Commands::Reset {
            all,
            daemon,
            dry_run,
        } => cmd_reset(config, all, daemon, dry_run, json),
    }
}

/// Generate a shell completion script for the given shell and write it to
/// stdout. Pure stdout output so operators can pipe straight into the
/// shell's completion-directory file (`/etc/bash_completion.d/abyss`,
/// `~/.config/fish/completions/abyss.fish`, etc.) without post-processing.
fn cmd_completion(shell: clap_complete::Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "abyss", &mut std::io::stdout());
    Ok(())
}

fn cmd_config(config: Config, action: ConfigCmd, json: bool) -> Result<()> {
    match action {
        ConfigCmd::Show => cmd_config_show(config, json),
    }
}

/// `abyss config show`: render the effective config + observable state so
/// operators and agents can sanity-check what abyss is actually running
/// without spelunking through `.code-abyss/`. Read-only by contract —
/// never opens the DB for writes, never mutates `arch.toml`, never talks
/// to the daemon's socket beyond a liveness probe.
///
/// Output shape:
///   * workspace / db / arch_toml_path / log_path
///   * schema_version (from `meta` table; 0 if no index yet)
///   * arch_toml: present + layer_rules + ignore_rules (when present)
///   * dictionary: built-in rule count
///   * index counts: files / chunks / symbols / refs (when DB exists)
///   * arch_layer_counts: per-layer file count from `arch_facts`
///   * daemon: { running, pid, uptime_secs } (best-effort)
fn cmd_config_show(config: Config, json: bool) -> Result<()> {
    use code_abyss::storage::Repository;

    let workspace = config.workspace.display().to_string();
    let db_path = config.db_path.display().to_string();
    let arch_toml_path = config
        .workspace
        .join(".code-abyss")
        .join("arch.toml")
        .display()
        .to_string();
    let log_path = config
        .workspace
        .join(".code-abyss")
        .join("daemon.log")
        .display()
        .to_string();
    let dictionary_builtin_rules = code_abyss::arch::builtin_rule_count();

    // arch.toml — best-effort. We use the existing loader so we surface the
    // exact same view the indexer sees (malformed TOML → "not loaded").
    let arch_toml = code_abyss::arch::load_overrides(&config.workspace);
    let arch_toml_view = match &arch_toml {
        Some(o) => serde_json::json!({
            "present": true,
            "path": arch_toml_path,
            "layer_rules": o.layer_rule_count(),
            "ignore_rules": o.ignore_rule_count(),
        }),
        None => serde_json::json!({
            "present": false,
            "path": arch_toml_path,
        }),
    };

    // Index facts — opening the repo is the only "expensive" step here.
    // Missing DB is a normal state for a fresh workspace; we surface it
    // explicitly rather than failing.
    let (schema_version, files, chunks, symbols, refs, arch_layer_counts) =
        if config.db_path.exists() {
            match Repository::open(&config.db_path, config.model.dimensions) {
                Ok(repo) => {
                    let conn = repo.conn();
                    let schema: i64 = conn
                        .query_row(
                            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    let files = repo.file_count().unwrap_or(0);
                    let chunks = repo.chunk_count().unwrap_or(0);
                    let symbols = repo.symbol_count().unwrap_or(0);
                    let refs = repo.ref_count().unwrap_or(0);

                    let mut by_layer: Vec<(String, i64)> = Vec::new();
                    let layer_query = conn.prepare(
                        "SELECT layer, COUNT(*) FROM arch_facts GROUP BY layer ORDER BY layer",
                    );
                    if let Ok(mut stmt) = layer_query
                        && let Ok(rows) = stmt
                            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    {
                        for row in rows.flatten() {
                            by_layer.push(row);
                        }
                    }
                    (schema, files, chunks, symbols, refs, by_layer)
                }
                Err(_) => (0, 0, 0, 0, 0, Vec::new()),
            }
        } else {
            (0, 0, 0, 0, 0, Vec::new())
        };
    let index_present = config.db_path.exists();
    let arch_layer_counts_json: serde_json::Value = arch_layer_counts
        .iter()
        .map(|(layer, count)| serde_json::json!({"layer": layer, "files": count}))
        .collect();

    // Daemon liveness — read pidfile + `kill(pid, 0)` on unix, omit on
    // platforms where the daemon doesn't exist. Best-effort: a stale
    // pidfile reports `running: false` so agents don't act on ghosts.
    let daemon_view = daemon_state_view(&config);

    let payload = serde_json::json!({
        "workspace": workspace,
        "db_path": db_path,
        "log_path": log_path,
        "schema_version": schema_version,
        "index_present": index_present,
        "arch_toml": arch_toml_view,
        "dictionary": {
            "builtin_rules": dictionary_builtin_rules,
        },
        "counts": {
            "files": files,
            "chunks": chunks,
            "symbols": symbols,
            "refs": refs,
        },
        "arch_layer_counts": arch_layer_counts_json,
        "daemon": daemon_view,
    });

    if json {
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    // TOML output — `toml::to_string_pretty` accepts any Serialize value
    // and renders tables/arrays in the multi-line form humans expect. The
    // single-line `toml::Value::Display` would jam the entire config onto
    // one row, defeating the point of inspecting it.
    match toml::to_string_pretty(&payload) {
        Ok(s) => {
            println!("# abyss config show");
            print!("{s}");
        }
        Err(_) => {
            // Defensive fallback — JSON is always valid output.
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

/// Best-effort daemon liveness summary used by `config show`. Returns a
/// `serde_json::Value` so callers can inline it into the payload without
/// caring about whether the pidfile / `kill(pid,0)` succeeded.
fn daemon_state_view(config: &Config) -> serde_json::Value {
    #[cfg(unix)]
    {
        use code_abyss::daemon::DaemonPaths;
        let paths = DaemonPaths::from_config(config);
        if !paths.pid.exists() {
            return serde_json::json!({"running": false});
        }
        let pid_raw = std::fs::read_to_string(&paths.pid).ok();
        let pid: Option<u32> = pid_raw
            .as_deref()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let alive = match pid {
            // SAFETY: `kill(pid, 0)` is the standard liveness probe — no
            // signal sent, just returns 0 when the pid is reachable from
            // this process. Identical to the `daemon status` probe.
            Some(p) => {
                let rc = unsafe { libc::kill(p as libc::pid_t, 0) };
                rc == 0
            }
            None => false,
        };
        let uptime_secs = paths
            .pid
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs());
        serde_json::json!({
            "running": alive,
            "pid": pid,
            "uptime_secs": uptime_secs,
            "pidfile": paths.pid.display().to_string(),
            "socket": paths.socket.display().to_string(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        serde_json::json!({"running": false, "note": "daemon is Unix-only"})
    }
}

fn cmd_daemon(config: Config, action: DaemonCmd) -> Result<()> {
    use code_abyss::daemon::{DaemonAction, run};
    let mapped = match action {
        DaemonCmd::Start { foreground, detach } => DaemonAction::Start { foreground, detach },
        DaemonCmd::Stop => DaemonAction::Stop,
        DaemonCmd::Status => DaemonAction::Status,
        DaemonCmd::Logs { tail, follow } => DaemonAction::Logs { tail, follow },
    };
    run(config, mapped)
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

/// Indexable file extensions for the `--since` driver. Keep in sync with
/// `walker::is_indexable` — the goal is to pre-filter git's diff output
/// to what abyss would actually parse, so we don't spawn the pipeline
/// against `.png` or `.lock` files that the walker would silently drop
/// anyway. Out-of-sync is a soft failure (extra parse-then-skip), not
/// a correctness bug.
const INDEXABLE_EXTS: &[&str] = &[
    "rs", "py", "pyi", "js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx", "go", "java", "c",
    "h", "cpp", "cc", "cxx", "hpp", "hxx", "hh", "json", "toml", "yml", "yaml", "sh", "bash",
    "html", "htm", "css", "scss", "md", "markdown",
];

/// Run `git diff --name-only --diff-filter=...` twice (added/modified/renamed
/// vs deleted) against `<since>..HEAD` and split the output. Returns
/// `(changed_abs_paths, deleted_rel_paths)` already filtered to indexable
/// extensions so the caller doesn't have to.
fn git_diff_changeset(
    workspace: &std::path::Path,
    since: &str,
) -> Result<(Vec<std::path::PathBuf>, Vec<String>)> {
    use std::process::Command;
    let range = format!("{since}..HEAD");

    let run = |filter: &str| -> Result<Vec<String>> {
        let out = Command::new("git")
            .current_dir(workspace)
            .args([
                "diff",
                "--name-only",
                &format!("--diff-filter={filter}"),
                &range,
            ])
            .output()
            .with_context(|| format!("git diff --diff-filter={filter}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "git diff exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    };

    let is_indexable = |p: &str| {
        std::path::Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| INDEXABLE_EXTS.contains(&ext))
    };

    // AMR: added / modified / renamed → reindex from the worktree.
    let changed_rel = run("AMR")?;
    let changed: Vec<std::path::PathBuf> = changed_rel
        .into_iter()
        .filter(|p| is_indexable(p))
        .map(|p| workspace.join(p))
        .collect();

    // D: deleted → drop from the index. Keep these workspace-relative so
    // they match the `files.path` column directly.
    let deleted: Vec<String> = run("D")?.into_iter().filter(|p| is_indexable(p)).collect();

    Ok((changed, deleted))
}

fn cmd_index(
    mut config: Config,
    json: bool,
    force: bool,
    max_files: Option<u64>,
    index_generated: bool,
    since: Option<String>,
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

    // `--since <ref>`: ask git for the change set. Falls back to a full
    // walk (with a stderr warning) when the workspace isn't a git repo —
    // the operator's intent is "index", not "fail on a missing optimization".
    if let Some(ref reference) = since {
        if !has_git {
            eprintln!(
                "abyss index --since: workspace is not a git repo — falling back to full walk"
            );
        } else {
            match git_diff_changeset(&config.workspace, reference) {
                Ok((changed, deleted)) => {
                    eprintln!(
                        "abyss index --since {reference}: {} changed, {} deleted",
                        changed.len(),
                        deleted.len()
                    );
                    pipeline.set_restrict_paths(changed, deleted);
                }
                Err(e) => {
                    // A bad ref shouldn't silently morph into a full walk —
                    // the operator picked --since deliberately. Surface and
                    // bail so they catch a typo before it eats CPU.
                    anyhow::bail!("abyss index --since {reference}: {e}");
                }
            }
        }
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

#[allow(clippy::too_many_arguments)]
fn cmd_callers(
    config: Config,
    symbol: &str,
    limit: usize,
    min_confidence: f64,
    include_tests: bool,
    calls_only: bool,
    types_only: bool,
    inherits_only: bool,
    _all_deps: bool,
    json: bool,
) -> Result<()> {
    use code_abyss::graph::CallerKindFilter;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    // `--all-deps` is a self-documenting alias for the default — it
    // collapses to the same `Both` branch as "no flag at all". Kept as a
    // dedicated arg so scripts can spell intent and so clap's
    // `conflicts_with_all` machinery enforces mutual exclusion with the
    // restricting flags.
    let kind_filter = match (calls_only, types_only, inherits_only) {
        (true, false, false) => CallerKindFilter::CallsOnly,
        (false, true, false) => CallerKindFilter::TypesOnly,
        (false, false, true) => CallerKindFilter::InheritsOnly,
        _ => CallerKindFilter::Both,
    };
    let restricted = calls_only || types_only || inherits_only;
    // `--limit 0` → unlimited (capped internally at UNLIMITED_CAP so a hot
    // framework primitive — Django Model, hono Context — can't OOM the
    // process). Two orders of magnitude above any caller list seen in
    // dogfood (Django Model topped at ~1k).
    const UNLIMITED_CAP: usize = 50_000;
    let effective_limit = if limit == 0 { UNLIMITED_CAP } else { limit };
    let result = gq.find_callers_filtered_kinds(
        symbol,
        effective_limit,
        min_confidence,
        include_tests,
        kind_filter,
    )?;
    // Total visible count — for the "showing N of M" footer (B3). Counted
    // server-side under the same filter so M honestly matches the view; test
    // callers are not counted toward M when include_tests=false (they're
    // invisible to the agent here).
    let total_available = repo.count_callers_at(
        symbol,
        kind_filter.as_slice(),
        min_confidence,
        include_tests,
    )?;
    let shown = result.callers.len();
    let was_capped = shown < total_available;

    if json {
        // Augment JSON with the total + cap info so MCP / scripts can
        // reproduce the footer without re-querying.
        let payload = serde_json::json!({
            "callers": result.callers,
            "excluded_tests": result.excluded_tests,
            "total_available": total_available,
            "limit": limit,
            "was_capped": was_capped,
        });
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        if result.callers.is_empty() && result.excluded_tests == 0 {
            eprintln!("no callers found for '{symbol}'");
            return Ok(());
        }
        // Header surfaces the test-exclusion contract so an agent that sees an
        // empty or short list knows whether to retry with --include-tests.
        let header = if include_tests {
            format!("callers of '{symbol}' ({} found):\n", result.callers.len())
        } else if result.excluded_tests > 0 {
            format!(
                "callers of '{symbol}' ({} prod, {} tests excluded — use --include-tests to see all):\n",
                result.callers.len(),
                result.excluded_tests
            )
        } else {
            format!("callers of '{symbol}' ({} prod):\n", result.callers.len())
        };
        println!("{header}");
        // When the user did not restrict via a flag, the list may mix call,
        // type_ref, and inherit edges. Suffix each row with the edge kind so
        // the agent can tell "X() invokes this" from "X uses this in a type
        // position" from "X inherits from this" without re-querying. When
        // restricted, the kind is implicit — keep the legacy compact format.
        for (i, c) in result.callers.iter().enumerate() {
            let t = if c.is_test { " [test]" } else { "" };
            let kind_suffix = if restricted {
                String::new()
            } else {
                format!(", {}", short_kind(&c.kind))
            };
            println!(
                "  {}. {}:{} → {}(){t}  ({:.0}%{kind_suffix})",
                i + 1,
                c.file_path,
                c.line + 1,
                c.symbol,
                c.confidence * 100.0,
            );
        }
        // Capped-list footer (B3). hono dogfood (2026-06-17): `callers
        // Context` showed 20 but 235 refs existed — no footer meant the
        // agent stopped reading at 20 and missed the real call sites.
        if was_capped {
            if limit == 0 {
                println!(
                    "\n(showing {shown} of {total_available} total — hit the {UNLIMITED_CAP} safety cap)"
                );
            } else {
                println!(
                    "\n(showing {shown} of {total_available} total — use --limit 0 for all, --limit N for more)"
                );
            }
        }
    }
    Ok(())
}

/// Short edge-kind label for the mixed callers listing: `call`, `field`,
/// `type`. Unknown kinds round-trip verbatim so a future edge type doesn't
/// silently masquerade as `call`.
fn short_kind(kind: &str) -> &str {
    match kind {
        "call" => "call",
        "field_access" => "field",
        "type_ref" => "type",
        other => other,
    }
}

fn cmd_impact(
    config: Config,
    symbol: &str,
    depth: u32,
    min_confidence: f64,
    calls_only: bool,
    json: bool,
) -> Result<()> {
    use code_abyss::graph::CallerKindFilter;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = code_abyss::graph::GraphQuery::new(&repo);
    // Default to the agent-facing superset (matches `abyss callers`).
    // `--calls-only` reverts to the v0.5.1 legacy "who invokes this function"
    // behaviour for users who want the function-only blast radius.
    let kind_filter = if calls_only {
        CallerKindFilter::CallsOnly
    } else {
        CallerKindFilter::Both
    };
    let result = gq.impact_analysis_filtered(symbol, depth, min_confidence, kind_filter)?;

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

/// Minimum git history we need before the hotspot ranking is informative.
/// Below this the change_count_30d signal is mostly zero (shallow clone or
/// fresh repo) and the empty list reads as "no risk" instead of "no data".
const MAP_MIN_COMMITS_FOR_HOTSPOTS: i64 = 10;

/// Hint to print under an empty Hotspots heading. Pure function so tests can
/// pin the string without spinning up a binary. Returns `None` when the
/// hotspot list is non-empty (the heading already has content).
///
/// Two failure modes deserve distinct hints:
/// * shallow / fresh repo (`total_commits < MAP_MIN_COMMITS_FOR_HOTSPOTS`)
///   → "insufficient history" so the agent knows to try a deeper clone
/// * historical repo with no recent activity → "no files changed in the
///   last 30 days" so the agent doesn't keep retrying / blames the index
fn empty_hotspots_hint(is_empty: bool, total_commits: i64) -> Option<String> {
    if !is_empty {
        return None;
    }
    Some(if total_commits < MAP_MIN_COMMITS_FOR_HOTSPOTS {
        format!(
            "  (insufficient history: {total_commits} commits available; need ≥{MAP_MIN_COMMITS_FOR_HOTSPOTS} — try a deeper clone)"
        )
    } else {
        String::from(
            "  (no files changed in the last 30 days — try `abyss history <path>` for older activity)",
        )
    })
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
        if hotspots.is_empty() {
            // Distinguishing "no risk" from "no data" matters: on a shallow
            // clone the empty hotspot list reads as a green light. Mirror
            // the coupling suppression message style so the two failure
            // modes look related.
            let total_commits: i64 = repo
                .conn()
                .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
                .unwrap_or(0);
            if let Some(hint) = empty_hotspots_hint(true, total_commits) {
                println!("{hint}");
            }
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

/// Scope of an `abyss reset` invocation. The CLI flags map onto these
/// branches once — keep the per-mode path resolution in `reset_targets`
/// so dry-run and the actual unlink stay byte-identical.
#[derive(Clone, Copy)]
enum ResetScope {
    /// Default: remove the index DB only, preserve `arch.toml` and any
    /// hand-authored config. The operator's intent is "rebuild the index".
    Default,
    /// `--all`: nuke the entire `.code-abyss/` directory. True greenfield.
    All,
    /// `--daemon`: remove daemon.pid / daemon.sock / daemon.log only.
    Daemon,
}

fn cmd_reset(config: Config, all: bool, daemon: bool, dry_run: bool, json: bool) -> Result<()> {
    let scope = match (all, daemon) {
        (true, _) => ResetScope::All,
        (false, true) => ResetScope::Daemon,
        (false, false) => ResetScope::Default,
    };

    // Refuse while a daemon is alive — otherwise the running process would
    // recreate `daemon.sock` / `daemon.pid` mid-reset and operators end up
    // chasing ghosts. A stale pidfile (process gone) is *not* live and
    // counts as a normal cleanup target.
    if let Some(pid) = live_daemon_pid(&config) {
        anyhow::bail!(
            "abyss reset: daemon still running (pid {pid}). Stop it first: abyss daemon stop"
        );
    }

    let targets = reset_targets(&config, scope);
    // Filter to paths that actually exist so the output reads like a real
    // diff — "would remove" / "removed" on a missing file is noise.
    let existing: Vec<&std::path::PathBuf> = targets.iter().filter(|p| p.exists()).collect();

    if existing.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({"removed": [], "dry_run": dry_run, "scope": scope_label(scope)})
            );
        } else {
            eprintln!("abyss reset: nothing to remove ({})", scope_label(scope));
        }
        return Ok(());
    }

    let mut removed: Vec<String> = Vec::new();
    for path in &existing {
        let display = path.display().to_string();
        if dry_run {
            if !json {
                println!("would remove: {display}");
            }
        } else {
            // Directory vs file: `.code-abyss/` is a dir; everything else
            // we own is a regular file. Skipping the dir-detect probe would
            // make `--all` fail with "Is a directory" on the unlink call.
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(_) => {
                    if !json {
                        println!("removed: {display}");
                    }
                }
                Err(e) => {
                    anyhow::bail!("abyss reset: failed to remove {display}: {e}");
                }
            }
        }
        removed.push(display);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "removed": removed,
                "dry_run": dry_run,
                "scope": scope_label(scope),
            })
        );
    }
    Ok(())
}

/// Resolve every filesystem path implicated by `scope`. Pure function so
/// the dry-run output and the actual removal traverse the identical list —
/// there can't be a target the unlink path forgets that dry-run promised.
fn reset_targets(config: &Config, scope: ResetScope) -> Vec<std::path::PathBuf> {
    let abyss_dir = config.workspace.join(".code-abyss");
    match scope {
        ResetScope::All => vec![abyss_dir],
        ResetScope::Default => {
            // The DB path is configurable (CLI `--db`), but the in-tree
            // default lives next to arch.toml. We rely on the resolved
            // `config.db_path` so a custom --db still gets nuked.
            vec![config.db_path.clone()]
        }
        ResetScope::Daemon => {
            vec![
                abyss_dir.join("daemon.pid"),
                abyss_dir.join("daemon.sock"),
                abyss_dir.join("daemon.log"),
            ]
        }
    }
}

fn scope_label(scope: ResetScope) -> &'static str {
    match scope {
        ResetScope::Default => "index-db",
        ResetScope::All => "all",
        ResetScope::Daemon => "daemon",
    }
}

/// Returns the live daemon pid if one is running against this workspace.
/// A pidfile without a backing process is *not* live — we want operators
/// to be able to `abyss reset` after an `kill -9` left a stale pid behind.
fn live_daemon_pid(config: &Config) -> Option<u32> {
    #[cfg(unix)]
    {
        use code_abyss::daemon::DaemonPaths;
        let paths = DaemonPaths::from_config(config);
        if !paths.pid.exists() {
            return None;
        }
        let pid: u32 = std::fs::read_to_string(&paths.pid)
            .ok()?
            .trim()
            .parse()
            .ok()?;
        // SAFETY: `kill(pid, 0)` is the standard liveness probe — no signal
        // delivered, just a permission/existence check via the kernel.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        alive.then_some(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        None
    }
}

fn cmd_attach(host: &str, local: bool) -> Result<()> {
    match host {
        "claude" => code_abyss::attach::claude::install(local),
        other => anyhow::bail!("unknown host: {other}; supported: claude"),
    }
}

fn cmd_watch(config: Config, debounce_ms: u64) -> Result<()> {
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
    let embedder: Option<Embedder> = match Embedder::load(&config.model) {
        Ok(e) => Some(e),
        Err(e) => {
            eprintln!("[abyss watch] embedder unavailable ({e}) — structural only");
            None
        }
    };
    #[cfg(not(feature = "semantic"))]
    let embedder: Option<code_abyss::embedding::Embedder> = None;

    let debounce = std::time::Duration::from_millis(debounce_ms);
    let watcher = code_abyss::watcher::FileWatcher::new(config.clone()).with_debounce(debounce);

    eprintln!(
        "abyss watching {} (debounce {}ms) — Ctrl-C to stop",
        config.workspace.display(),
        debounce_ms
    );
    watcher.watch(&repo, embedder.as_ref(), &pipeline)?;
    Ok(())
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
fn cmd_mcp_via_daemon(config: Config) -> Result<()> {
    use code_abyss::daemon::DaemonPaths;
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
fn cmd_mcp_via_daemon(_config: Config) -> Result<()> {
    anyhow::bail!("`abyss mcp --via-daemon` is Unix-only; use `abyss mcp` for standalone mode");
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

#[cfg(test)]
mod map_hint_tests {
    //! Pins the empty-hotspots hint contract. Two failure modes ("no data"
    //! vs "no recent activity") must render as distinct, agent-readable
    //! strings, mirroring the coupling suppression style.
    use super::{MAP_MIN_COMMITS_FOR_HOTSPOTS, empty_hotspots_hint};

    #[test]
    fn non_empty_hotspots_get_no_hint() {
        assert!(empty_hotspots_hint(false, 0).is_none());
        assert!(empty_hotspots_hint(false, 1000).is_none());
    }

    #[test]
    fn shallow_clone_renders_insufficient_history() {
        let hint = empty_hotspots_hint(true, 3).expect("hint expected");
        assert!(
            hint.contains("insufficient history"),
            "want shallow-clone hint, got: {hint}",
        );
        assert!(
            hint.contains("3 commits available"),
            "should surface the actual count, got: {hint}",
        );
        assert!(
            hint.contains(&format!("≥{MAP_MIN_COMMITS_FOR_HOTSPOTS}")),
            "should surface the required threshold, got: {hint}",
        );
    }

    #[test]
    fn historical_repo_no_recent_activity_renders_separate_hint() {
        // Plenty of commits, just none in the last 30 days. We must NOT
        // say "insufficient history" — the agent would chase a deeper
        // clone for nothing.
        let hint = empty_hotspots_hint(true, 500).expect("hint expected");
        assert!(
            hint.contains("no files changed in the last 30 days"),
            "want no-recent-activity hint, got: {hint}",
        );
        assert!(
            !hint.contains("insufficient history"),
            "must not blame history when commits exist: {hint}",
        );
    }

    #[test]
    fn threshold_boundary_is_lt_not_le() {
        // At exactly the threshold we treat the repo as "enough history":
        // the empty list means "no recent change", not "shallow clone".
        let below = empty_hotspots_hint(true, MAP_MIN_COMMITS_FOR_HOTSPOTS - 1).unwrap();
        assert!(below.contains("insufficient history"), "{below}");
        let at = empty_hotspots_hint(true, MAP_MIN_COMMITS_FOR_HOTSPOTS).unwrap();
        assert!(
            !at.contains("insufficient history"),
            "boundary must flip to 'no recent activity', got: {at}",
        );
    }
}
