//! Shared daemon state surfaced over the socket protocol.
//!
//! Kept deliberately small for V1 — pid, start time, last-reindex telemetry,
//! and a paths handle so the socket layer can read counts directly from the
//! same DB the watcher writes to.

use std::path::PathBuf;
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
}

impl DaemonState {
    pub fn new(pid: u32, config: &Config) -> Self {
        Self {
            pid,
            started_at: Instant::now(),
            last_reindex_ms: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            db_path: config.db_path.clone(),
            workspace: config.workspace.clone(),
            dimensions: config.model.dimensions,
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
