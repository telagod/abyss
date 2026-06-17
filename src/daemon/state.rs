//! Shared daemon state surfaced over the socket protocol.
//!
//! Kept deliberately small for V1 — pid, start time, last-reindex telemetry,
//! and a paths handle so the socket layer can read counts directly from the
//! same DB the watcher writes to.
//!
//! V1.5 additions:
//! - Full `Config` reference so the socket layer can build its own
//!   [`IndexPipeline`] when the `reindex` verb arrives.
//! - `reindex_lock` — a `try_lock`-gated mutex that serializes operator-
//!   triggered reindex requests against each other. The watcher itself
//!   doesn't honor it (it's the canonical writer, runs only on debounced
//!   FS events, and concurrent SQLite WAL writes are short enough to ride
//!   out via SQLite's own busy handler), so a `reindex` verb arriving
//!   mid-debounce burst can still briefly contend at the SQLite layer —
//!   that's acceptable for V1.5 and documented in the daemon README.
//! - `log_path` — so the `logs` verb can `tail -n` the daemon log without
//!   the socket layer re-deriving paths from `Config`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::config::Config;

pub struct DaemonState {
    pub pid: u32,
    pub started_at: Instant,
    /// Wall-clock ms elapsed during the last incremental reindex burst.
    /// `0` means "no reindex observed yet since startup".
    pub last_reindex_ms: AtomicU64,
    /// Monotonic counter bumped on every reindex — clients can use this to
    /// detect "anything changed since I last looked" without polling content.
    pub epoch: AtomicU64,
    /// SQLite DB the daemon is watching — handed off to the socket layer so
    /// `stats` can be answered without taking a lock on the watcher's repo.
    pub db_path: PathBuf,
    pub workspace: PathBuf,
    /// Dimensions for opening the read-only repo handle in the socket layer.
    pub dimensions: usize,
    /// Full config — handed to the socket worker so it can spin up its own
    /// [`crate::indexer::IndexPipeline`] on the `reindex` verb without
    /// re-deriving workspace flags.
    pub config: Config,
    /// Path to the daemon log file — fed to the `logs` verb's tail helper.
    pub log_path: PathBuf,
    /// Operator-reindex serialization. `try_lock`'d by the `reindex` verb so
    /// two concurrent socket requests don't both try to write — the loser
    /// gets a structured `lock contention` error rather than a SQLite BUSY.
    pub reindex_lock: Mutex<()>,
}

impl DaemonState {
    pub fn new(pid: u32, config: &Config, log_path: PathBuf) -> Self {
        Self {
            pid,
            started_at: Instant::now(),
            last_reindex_ms: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            db_path: config.db_path.clone(),
            workspace: config.workspace.clone(),
            dimensions: config.model.dimensions,
            config: config.clone(),
            log_path,
            reindex_lock: Mutex::new(()),
        }
    }

    pub fn record_reindex(&self, ms: u64) {
        self.last_reindex_ms.store(ms, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
