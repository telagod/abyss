//! `abyss index --since <ref>` contract.
//!
//! Set up a tempdir git repo with three files, commit, modify one,
//! commit again, then run `abyss index --since HEAD~1` from a fresh
//! state. The hash log proves only the modified file went through the
//! reindex path — the two unchanged files keep their hash from the
//! initial index pass.
//!
//! Linux-only — keeps the git-shell-out lane deterministic.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Pull the hash of a single file from the index so the test can
/// distinguish "rewritten with identical content" from "left alone" —
/// the structural pipeline upserts a new row with the same hash on a
/// changed file, so a stable hash means the per-file reindex really did
/// skip the other paths.
fn file_hash(repo: &Repository, rel: &str) -> Option<String> {
    let mut stmt = repo
        .conn()
        .prepare("SELECT hash FROM files WHERE path = ?1")
        .expect("prepare");
    stmt.query_row([rel], |r| r.get::<_, String>(0)).ok()
}

#[test]
fn since_reindexes_only_changed_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");

    // Init repo + identity so commits succeed in CI.
    git(&ws, &["init", "--initial-branch=main", "-q"]);
    git(&ws, &["config", "user.email", "t@example.com"]);
    git(&ws, &["config", "user.name", "tester"]);

    std::fs::write(ws.join("a.go"), "package main\n\nfunc A() {}\n").unwrap();
    std::fs::write(ws.join("b.go"), "package main\n\nfunc B() {}\n").unwrap();
    std::fs::write(ws.join("c.go"), "package main\n\nfunc C() {}\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-q", "-m", "initial"]);

    // Seed the index against the initial state.
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).expect("initial index");
    }

    // Capture pre-modification hashes so we can prove non-targets stayed put.
    let (a_pre, b_pre, c_pre) = {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
        (
            file_hash(&repo, "a.go").expect("a.go"),
            file_hash(&repo, "b.go").expect("b.go"),
            file_hash(&repo, "c.go").expect("c.go"),
        )
    };

    // Mutate one file and commit so HEAD~1 names the right baseline.
    std::fs::write(ws.join("b.go"), "package main\n\nfunc B() { _ = 1 }\n").unwrap();
    git(&ws, &["add", "b.go"]);
    git(&ws, &["commit", "-q", "-m", "edit b"]);

    // --since HEAD~1 should reindex b.go only.
    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("index")
        .arg("--since")
        .arg("HEAD~1")
        .output()
        .expect("spawn abyss index --since");
    assert!(
        out.status.success(),
        "index --since failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--since HEAD~1") || stderr.contains("1 changed"),
        "expected --since summary in stderr, got: {stderr}"
    );

    let (a_post, b_post, c_post) = {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
        (
            file_hash(&repo, "a.go").expect("a.go"),
            file_hash(&repo, "b.go").expect("b.go"),
            file_hash(&repo, "c.go").expect("c.go"),
        )
    };

    assert_eq!(a_pre, a_post, "a.go must not be touched by --since HEAD~1");
    assert_ne!(b_pre, b_post, "b.go must be reindexed (content changed)");
    assert_eq!(c_pre, c_post, "c.go must not be touched by --since HEAD~1");
}

#[test]
fn since_handles_deleted_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");
    git(&ws, &["init", "--initial-branch=main", "-q"]);
    git(&ws, &["config", "user.email", "t@example.com"]);
    git(&ws, &["config", "user.name", "tester"]);

    std::fs::write(ws.join("keep.go"), "package main\n\nfunc K() {}\n").unwrap();
    std::fs::write(ws.join("drop.go"), "package main\n\nfunc D() {}\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-q", "-m", "initial"]);

    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).expect("initial");
    }

    // Confirm drop.go is in the index pre-removal.
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
        assert!(
            file_hash(&repo, "drop.go").is_some(),
            "drop.go must be indexed first"
        );
    }

    git(&ws, &["rm", "-q", "drop.go"]);
    git(&ws, &["commit", "-q", "-m", "remove drop"]);

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("index")
        .arg("--since")
        .arg("HEAD~1")
        .output()
        .expect("spawn abyss index --since");
    assert!(
        out.status.success(),
        "since failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let repo = Repository::open(&config.db_path, config.model.dimensions).expect("repo");
    assert!(
        file_hash(&repo, "drop.go").is_none(),
        "deleted path must be dropped from the index by --since"
    );
    assert!(
        file_hash(&repo, "keep.go").is_some(),
        "untouched path must stay in the index"
    );
}
