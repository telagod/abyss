//! CLI command handlers extracted from main.rs.
//!
//! Each submodule groups related `cmd_*` functions by domain. The top-level
//! `Cli` and `Commands` structs remain in `main.rs` since they own the
//! clap dispatch logic. Sub-enums that command handlers need to
//! pattern-match on live here so both sides can see them.

use clap::Subcommand;

pub mod attach;
pub mod daemon;
pub mod hooks;
pub mod index;
pub mod inspect;
pub mod proxy;
pub mod query;

// ---------------------------------------------------------------------------
// Clap sub-enums — shared between main.rs dispatch and command handlers
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum HookAction {
    /// Pre-edit guard: emit a `<abyss-card>` system-reminder block on stderr
    /// for the file referenced in the tool-call JSON on stdin. Read-only —
    /// never re-indexes (use post-edit for that).
    PreEdit,
    /// Post-edit: incrementally refresh the index
    PostEdit,
    /// Proxy rewrite: intercept a Bash tool call, rewrite the command to
    /// route through `abyss proxy`, and return the hook response JSON.
    /// Used by `abyss attach --proxy` hooks.
    ProxyRewrite,
}

#[derive(Subcommand)]
pub enum IngestCmd {
    /// Ingest a SCIP index file. v0.5.15 prototype: only
    /// `--dry-run --print-summary` against a `scip print --json` blob
    /// is supported; full DB writes land in a follow-up patch.
    Scip {
        /// Path to a `scip print --json` JSON file. Binary `.scip`
        /// inputs error out for now with a pointer to the conversion
        /// command (`scip print --json <file>.scip > /tmp/index.json`).
        path: std::path::PathBuf,
        /// Parse the input and print the summary without writing to
        /// the index DB. Required in v0.5.15 — the real ingest path
        /// is not wired yet, so dropping `--dry-run` is also a no-op
        /// today. Kept on the CLI so scripts can pin the future shape.
        #[arg(long)]
        dry_run: bool,
        /// Print document / occurrence / ref-candidate counts on
        /// stderr (or JSON via the global `--json` flag). The summary
        /// is the contract the v0.5.16 MCP `ingest_scip` tool will
        /// surface verbatim.
        #[arg(long)]
        print_summary: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Print effective config + state. TOML by default; `--json` for
    /// machine consumption. Read-only — never mutates state.
    Show,
}

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Acquire pidfile lock, bind socket, start watching. Use `--detach` to
    /// double-fork into the background; `--foreground` keeps the daemon
    /// attached to the controlling terminal (overrides `--detach`).
    Start {
        /// Stay attached to the controlling terminal (don't redirect logs).
        #[arg(long)]
        foreground: bool,
        /// Detach via double-fork + setsid. The shell returns once the
        /// child has claimed the pidfile (<=500ms).
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
