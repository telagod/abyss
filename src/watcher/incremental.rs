use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::indexer::IndexPipeline;
use crate::indexer::parser;
use crate::storage::Repository;

/// Default debounce window: 150ms matches the "tier-A debounce" target —
/// fast enough that a save → reindex feels instant, slow enough to coalesce
/// editor write bursts (e.g. atomic-rename saves) into a single update.
pub const DEFAULT_DEBOUNCE_MS: u64 = 150;

/// Polling tick for the receiver loop. Keeps Ctrl-C / stop-signal responsive
/// without busy-spinning.
const POLL_TICK: Duration = Duration::from_millis(200);

/// Callback fired after each incremental reindex batch completes.
/// Receives the wall-clock milliseconds the batch took. Used by the daemon
/// to surface "last reindex" telemetry over the socket; foreground `abyss
/// watch` callers can ignore it.
pub type ReindexCallback = Box<dyn Fn(u64) + Send + Sync>;

pub struct FileWatcher {
    config: Config,
    debounce: Duration,
    on_reindex: Option<ReindexCallback>,
}

impl FileWatcher {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            on_reindex: None,
        }
    }

    /// Override the debounce window. Use [`FileWatcher::new`] for the default
    /// (150ms — matches the daemon-lite blueprint).
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Register a callback to be invoked after every reindex batch with the
    /// elapsed milliseconds. The daemon uses this to keep its socket-exposed
    /// `last_reindex_ms` field current without the watcher knowing about the
    /// daemon.
    pub fn with_on_reindex<F>(mut self, cb: F) -> Self
    where
        F: Fn(u64) + Send + Sync + 'static,
    {
        self.on_reindex = Some(Box::new(cb));
        self
    }

    /// Watch the workspace and incrementally reindex changed files until the
    /// debounce channel disconnects (process termination / dropped watcher).
    ///
    /// In slim builds pass `None` for the embedder — semantic-only paths are
    /// skipped. Semantic builds pass `Some(&embedder)` to keep vectors fresh.
    pub fn watch(
        &self,
        repo: &Repository,
        embedder: Option<&Embedder>,
        pipeline: &IndexPipeline,
    ) -> Result<()> {
        self.watch_with_cancel(repo, embedder, pipeline, None)
    }

    /// Same as [`FileWatcher::watch`] but accepts a cancel receiver so tests
    /// (and future explicit stop paths) can break the loop deterministically.
    /// A unit-payload send on `stop_rx` triggers a clean shutdown.
    pub fn watch_with_cancel(
        &self,
        repo: &Repository,
        embedder: Option<&Embedder>,
        pipeline: &IndexPipeline,
        stop_rx: Option<mpsc::Receiver<()>>,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel::<DebounceEventResult>();

        // notify-debouncer-full coalesces a burst of FS events into one callback
        // per debounce window — no hand-rolled timeout loop needed.
        let mut debouncer = new_debouncer(self.debounce, None, tx)?;
        debouncer.watch(&self.config.workspace, RecursiveMode::Recursive)?;
        info!(
            "watching {} for changes (debounce {:?})",
            self.config.workspace.display(),
            self.debounce
        );

        loop {
            if let Some(rx) = stop_rx.as_ref()
                && rx.try_recv().is_ok()
            {
                debug!("watcher: stop signal received");
                break;
            }

            match rx.recv_timeout(POLL_TICK) {
                Ok(Ok(events)) => {
                    let start = Instant::now();
                    let mut pending: std::collections::HashSet<PathBuf> =
                        std::collections::HashSet::new();
                    let mut removed: Vec<PathBuf> = Vec::new();

                    for event in events {
                        for path in &event.paths {
                            if !self.should_index(path) {
                                continue;
                            }
                            match event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    pending.insert(path.clone());
                                }
                                EventKind::Remove(_) => {
                                    removed.push(path.clone());
                                }
                                _ => {}
                            }
                        }
                    }

                    if pending.is_empty() && removed.is_empty() {
                        continue;
                    }

                    // Removes are handled per-path: the file no longer exists,
                    // so cascade-delete its rows + drop incoming refs to NULL.
                    if !removed.is_empty() {
                        repo.begin_transaction().ok();
                        for path in &removed {
                            if let Err(e) = pipeline.remove_file(repo, path) {
                                warn!("failed to remove {}: {e}", path.display());
                            } else {
                                debug!("removed: {}", path.display());
                            }
                        }
                        repo.commit().ok();
                    }

                    // Modifies go through the full structural pipeline because
                    // refs only get re-resolved in the batch resolver — a
                    // per-file reindex would silently drop the file's outgoing
                    // call edges. `run_structural` is hash-incremental: it
                    // skips unchanged files, so the cost stays proportional
                    // to what actually changed.
                    if !pending.is_empty() {
                        match pipeline.run_structural(repo) {
                            Ok(stats) => debug!(
                                "structural rerun: {} files, {} refs in {}ms",
                                stats.total_files, stats.refs, stats.duration_ms
                            ),
                            Err(e) => warn!("structural rerun failed: {e}"),
                        }

                        // Semantic builds: refresh vectors only for the files
                        // that changed (the structural batch doesn't touch
                        // embeddings, so this is the seam where they catch up).
                        if let Some(emb) = embedder {
                            for path in &pending {
                                if let Err(e) = pipeline.reindex_file(repo, Some(emb), path) {
                                    warn!("embed refresh failed {}: {e}", path.display());
                                }
                            }
                        }
                    }

                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    info!(
                        "incremental update: {} reindexed, {} removed in {}ms",
                        pending.len(),
                        removed.len(),
                        elapsed_ms
                    );
                    if let Some(cb) = self.on_reindex.as_ref() {
                        cb(elapsed_ms);
                    }
                }
                Ok(Err(errors)) => {
                    for e in errors {
                        warn!("watch error: {e}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        Ok(())
    }

    fn should_index(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        if path_str.contains(".code-abyss")
            || path_str.contains(".git")
            || path_str.contains("node_modules")
            || path_str.contains("target/")
        {
            return false;
        }

        parser::detect_language(&path_str).is_some()
    }
}
