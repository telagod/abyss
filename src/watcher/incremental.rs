use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::indexer::IndexPipeline;
use crate::indexer::parser;
use crate::storage::Repository;

pub struct FileWatcher {
    config: Config,
}

impl FileWatcher {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn watch(
        &self,
        repo: &Repository,
        embedder: &Embedder,
        pipeline: &IndexPipeline,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    let _ = tx.send(event);
                }
                Err(e) => warn!("watch error: {e}"),
            })?;

        watcher.watch(self.config.workspace.as_ref(), RecursiveMode::Recursive)?;
        info!("watching {} for changes", self.config.workspace.display());

        let debounce = Duration::from_millis(500);
        let mut pending: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut last_event = std::time::Instant::now();

        loop {
            match rx.recv_timeout(debounce) {
                Ok(event) => {
                    for path in &event.paths {
                        if self.should_index(path) {
                            match event.kind {
                                EventKind::Create(_) | EventKind::Modify(_) => {
                                    pending.insert(path.clone());
                                }
                                EventKind::Remove(_) => {
                                    repo.begin_transaction().ok();
                                    if let Err(e) = pipeline.remove_file(repo, path) {
                                        warn!("failed to remove {}: {e}", path.display());
                                    }
                                    repo.commit().ok();
                                    debug!("removed: {}", path.display());
                                }
                                _ => {}
                            }
                        }
                    }
                    last_event = std::time::Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !pending.is_empty() && last_event.elapsed() >= debounce {
                        let paths: Vec<PathBuf> = pending.drain().collect();
                        repo.begin_transaction().ok();
                        for path in &paths {
                            match pipeline.reindex_file(repo, embedder, path) {
                                Ok(n) => debug!("reindexed {} ({n} chunks)", path.display()),
                                Err(e) => warn!("reindex failed {}: {e}", path.display()),
                            }
                        }
                        repo.commit().ok();
                        info!("incremental update: {} files", paths.len());
                    }
                }
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
