use std::path::PathBuf;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use code_abyss::commands::{ConfigCmd, DaemonCmd, HookAction, IngestCmd};
use code_abyss::config::Config;

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
    /// Supported hosts: claude, codex, gemini, openclaw, all
    Attach {
        /// Agent host: claude | codex | gemini | openclaw | all
        host: String,
        /// Write to <cwd>/.<host>/<settings file> instead of $HOME
        #[arg(long)]
        local: bool,
        /// Skip installing the proxy-rewrite hook (installed by default
        /// for supported hosts)
        #[arg(long)]
        no_proxy: bool,
    },
    /// One-command onboarding: index the workspace, install hooks into
    /// all detected agent hosts, and enable proxy compression.
    ///
    /// Equivalent to: `abyss index && abyss attach all`
    Setup {
        /// Write hooks to <cwd>/.<host>/ instead of $HOME
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
    /// Ingest SCIP (Source Code Intelligence Protocol) ground truth from
    /// an LSP-grade indexer. Prototype — v0.5.15 ships the wiring +
    /// `--dry-run --print-summary` against `scip print --json` output.
    /// Full DB ingest (promoting refs to confidence=1.0 as an L-1 tier)
    /// is the next iteration. Binary `.scip` files surface an actionable
    /// error pointing at `eval/setup-indexers.sh`.
    Ingest {
        #[command(subcommand)]
        action: IngestCmd,
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
    /// Emit a machine-readable skill manifest for skill-discovery
    /// consumers (e.g. the companion `code-abyss` package). Describes
    /// the CLI surface, MCP tools, hook entry points, daemon socket
    /// verbs — everything an integrating tool needs without hand-coding.
    ///
    /// Defaults to pretty-printed JSON; pass `--compact` for a single
    /// line suitable for machine pipelines.
    SkillManifest {
        /// Emit a single-line JSON payload instead of the default
        /// pretty-printed form.
        #[arg(long)]
        compact: bool,
    },
    /// Proxy a command: run it, compress its output, and print the
    /// compressed version. Token savings are tracked in the index DB.
    ///
    /// Examples:
    ///   abyss proxy git status
    ///   abyss proxy cargo test
    ///   abyss proxy --tee ls -la src/
    Proxy {
        /// Preserve full unfiltered output to `.code-abyss/tee/`
        /// (default: only on failures)
        #[arg(long)]
        tee: bool,
        /// Print which handler/filter was used and why on stderr
        #[arg(long)]
        explain: bool,
        /// The command and its arguments to proxy
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    /// Show token savings report from proxied commands.
    ///
    /// Reads the tracking table in the index DB and renders a summary
    /// of total tokens saved, avg compression ratio, and top commands.
    Gain {
        /// Number of days to look back (default: 30)
        #[arg(long, default_value_t = 30)]
        days: u32,
    },
    /// Rewrite a shell command for proxy interception. Used by hook
    /// scripts — not typically called directly.
    ///
    /// Exit codes: 0 = rewritten (stdout has the new command),
    /// 1 = no rewrite available (pass through as-is).
    Rewrite {
        /// The command string to rewrite
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
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

    use code_abyss::commands::{attach, daemon, hooks, index, inspect, proxy, query};

    match cli.command {
        Commands::Index {
            force,
            max_files,
            index_generated,
            since,
        } => index::cmd_index(config, json, force, max_files, index_generated, since),
        #[cfg(feature = "semantic")]
        Commands::Embed => index::cmd_embed(config),
        #[cfg(feature = "semantic")]
        Commands::IndexAll { watch } => index::cmd_index_all(config, watch),
        Commands::Search { query: q, limit } => query::cmd_search(config, &q, limit, json),
        Commands::Callers {
            symbol,
            limit,
            min_confidence,
            include_tests,
            calls_only,
            types_only,
            inherits_only,
            all_deps,
        } => query::cmd_callers(
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
        } => query::cmd_impact(config, &symbol, depth, min_confidence, calls_only, json),
        Commands::History { file, symbol } => {
            query::cmd_history(config, &file, symbol.as_deref(), json)
        }
        Commands::Context { file } => query::cmd_context(config, &file, json),
        Commands::Map { limit } => inspect::cmd_map(config, limit, json),
        Commands::Where { file } => query::cmd_where(config, &file, json),
        Commands::Stats => inspect::cmd_stats(config, json),
        Commands::Hook { action } => hooks::cmd_hook(config, action, json),
        Commands::Attach {
            host,
            local,
            no_proxy,
        } => attach::cmd_attach(&host, local, !no_proxy),
        Commands::Setup { local } => attach::cmd_setup(config, local, json),
        Commands::Mcp { via_daemon } => {
            if via_daemon {
                daemon::cmd_mcp_via_daemon(config)
            } else {
                daemon::cmd_mcp(config)
            }
        }
        Commands::Watch { debounce_ms } => daemon::cmd_watch(config, debounce_ms),
        Commands::Daemon { action } => daemon::cmd_daemon(config, action),
        Commands::Completion { shell } => query::cmd_completion(shell, &mut Cli::command()),
        Commands::Config { action } => match action {
            ConfigCmd::Show => inspect::cmd_config_show(config, json),
        },
        Commands::Reset {
            all,
            daemon: daemon_flag,
            dry_run,
        } => index::cmd_reset(config, all, daemon_flag, dry_run, json),
        Commands::Ingest { action } => attach::cmd_ingest(action, json),
        Commands::SkillManifest { compact } => attach::cmd_skill_manifest(compact),
        Commands::Proxy {
            tee,
            explain,
            command,
        } => proxy::cmd_proxy(config, command, tee, explain, json),
        Commands::Gain { days } => proxy::cmd_gain(config, days, json),
        Commands::Rewrite { command } => proxy::cmd_rewrite(command),
    }
}

#[cfg(test)]
mod map_hint_tests {
    //! Pins the empty-hotspots hint contract. Two failure modes ("no data"
    //! vs "no recent activity") must render as distinct, agent-readable
    //! strings, mirroring the coupling suppression style.
    use code_abyss::commands::inspect::{MAP_MIN_COMMITS_FOR_HOTSPOTS, empty_hotspots_hint};

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
