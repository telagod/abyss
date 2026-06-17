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
use std::sync::mpsc;
use std::time::Instant;

use serde::Serialize;

use crate::config::Config;

/// Push-notification payload broadcast to every active `subscribe`
/// connection after each watcher reindex burst. Wire shape is JSON,
/// newline-delimited — clients read one line per event.
///
/// Kept intentionally small: editor extensions polling for "is the
/// index fresh?" only need the epoch + reindex count; deeper inspection
/// goes through `stats` / `ping` on a separate connection.
#[derive(Debug, Clone, Serialize)]
pub struct ReindexEvent {
    /// Always `"reindexed"` for v0.5.16 — future event kinds (e.g.
    /// `"removed"`, `"error"`) can land as siblings without breaking
    /// existing subscribers.
    pub event: &'static str,
    /// File count touched in the burst (added + modified). Matches the
    /// watcher's own `info!` log so the event mirrors what an operator
    /// sees in `daemon logs`.
    pub files: u64,
    /// Monotonic counter from [`DaemonState::epoch`] after the bump.
    pub epoch: u64,
    /// Wall-clock seconds since the Unix epoch for the broadcast moment.
    /// Lets a subscriber dedupe rapid bursts without a local clock.
    pub ts: u64,
}

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
    /// Active subscribers to reindex push notifications (v0.5.16
    /// `subscribe` verb). Each `subscribe` handler thread holds the
    /// receiver end of an mpsc channel; the watcher's `on_reindex`
    /// callback iterates this list and fans out a [`ReindexEvent`] to
    /// every still-living sender. Sends are non-blocking — a slow
    /// subscriber that lets its channel fill is silently dropped on the
    /// next bump so the watcher thread can't be backpressured by a
    /// stuck client.
    pub subscribers: Mutex<Vec<mpsc::Sender<ReindexEvent>>>,
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
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub fn record_reindex(&self, ms: u64) {
        self.last_reindex_ms.store(ms, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }

    /// Register a new subscriber channel. Returns the receiver end the
    /// caller polls for [`ReindexEvent`] pushes. The sender end is held
    /// inside `subscribers` until the receiver drops — at which point
    /// the next broadcast naturally prunes the entry.
    pub fn register_subscriber(&self) -> mpsc::Receiver<ReindexEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut guard) = self.subscribers.lock() {
            guard.push(tx);
        }
        rx
    }

    /// Drop every subscriber sender. Causes any blocked
    /// `subscribe`-handler thread to wake with `Disconnected` on its
    /// next recv. Called from the daemon shutdown path so push
    /// subscribers tear down promptly instead of waiting out their
    /// 60-second keepalive timeout.
    pub fn clear_subscribers(&self) {
        if let Ok(mut guard) = self.subscribers.lock() {
            guard.clear();
        }
    }

    /// Fan-out a reindex notification to every subscriber. Dead
    /// receivers (peer dropped) are pruned in-place so the list stays
    /// bounded across long-running daemons. Send failures are not
    /// logged because they're the normal "client went away" path.
    pub fn broadcast_reindex(&self, files: u64) {
        let epoch = self.epoch.load(Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let event = ReindexEvent {
            event: "reindexed",
            files,
            epoch,
            ts,
        };
        let Ok(mut guard) = self.subscribers.lock() else {
            return;
        };
        guard.retain(|tx| tx.send(event.clone()).is_ok());
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
