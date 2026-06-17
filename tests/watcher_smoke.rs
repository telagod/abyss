//! Smoke test: file save → FileWatcher → incremental reindex.
//!
//! Spins up a tempdir workspace with one indexed Go file, starts the watcher
//! in a background thread, mutates the file, and verifies the symbol table
//! reflects the post-edit state.
//!
//! Notify is async + platform-sensitive, so the test is gated to Linux where
//! the inotify backend is deterministic enough for CI. On macOS/Windows the
//! fsevents/ReadDirectoryChangesW backends occasionally drop events under
//! load — they're fine in real use, just flaky in tight test loops.

#![cfg(target_os = "linux")]

mod common;

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;
use code_abyss::watcher::FileWatcher;

fn symbol_names(repo: &Repository, file_path: &str) -> Vec<String> {
    let conn = repo.conn();
    let mut stmt = conn
        .prepare(
            "SELECT s.name FROM symbols s
             JOIN files f ON s.file_id = f.id
             WHERE f.path = ?1
             ORDER BY s.name",
        )
        .unwrap();
    stmt.query_map([file_path], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect()
}

#[test]
fn file_save_triggers_incremental_reindex() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();

    let initial = "package main\n\nfunc Alpha() {}\n";
    std::fs::write(ws.join("main.go"), initial).unwrap();

    let config = Config::new(&ws);
    let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();

    let pre: Vec<String> = symbol_names(&repo, "main.go");
    assert!(
        pre.iter().any(|s| s == "Alpha"),
        "pre-edit symbols missing Alpha: {pre:?}"
    );
    assert!(
        !pre.iter().any(|s| s == "Beta"),
        "Beta should not exist yet: {pre:?}"
    );
    drop(repo);

    // Spawn the watcher with a tight debounce and a stop channel so the test
    // can shut it down cleanly.
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let watcher_config = config.clone();
    let handle = thread::spawn(move || {
        let repo = Repository::open(&watcher_config.db_path, watcher_config.model.dimensions)
            .expect("watcher: open repo");
        let pipeline = IndexPipeline::new(watcher_config.clone());
        let watcher = FileWatcher::new(watcher_config).with_debounce(Duration::from_millis(80));
        watcher
            .watch_with_cancel(&repo, None, &pipeline, Some(stop_rx))
            .expect("watcher: watch");
    });

    // Give the watcher a moment to install the inotify subscription.
    thread::sleep(Duration::from_millis(250));

    let updated = "package main\n\nfunc Alpha() {}\nfunc Beta() {}\n";
    std::fs::write(ws.join("main.go"), updated).unwrap();

    // Poll until the reindex lands or we time out. Debounce + commit + slack:
    // 80ms debounce + ~200ms poll tick + reindex overhead → comfortably under 3s.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut post: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
        post = symbol_names(&repo, "main.go");
        drop(repo);
        if post.iter().any(|s| s == "Beta") {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Signal shutdown and join (best-effort — the watcher polls stop every
    // POLL_TICK ≈ 200ms).
    let _ = stop_tx.send(());
    let _ = handle.join();

    assert!(
        post.iter().any(|s| s == "Alpha") && post.iter().any(|s| s == "Beta"),
        "post-edit symbols missing Beta after watcher reindex: {post:?}"
    );
}
