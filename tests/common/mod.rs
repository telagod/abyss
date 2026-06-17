#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;
use tempfile::TempDir;

pub struct Fixture {
    pub _dir: TempDir,
    pub repo: Repository,
    pub config: Config,
}

pub fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }
}

fn index_dir(dir: TempDir) -> Fixture {
    let ws = std::fs::canonicalize(dir.path()).unwrap();
    let config = Config::new(&ws);
    let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();
    Fixture {
        _dir: dir,
        repo,
        config,
    }
}

/// Write files into a fresh (non-git) workspace and run the structural index.
pub fn index_fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    write_files(dir.path(), files);
    index_dir(dir)
}

pub fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "abyss-test")
        .env("GIT_AUTHOR_EMAIL", "t@test")
        .env("GIT_COMMITTER_NAME", "abyss-test")
        .env("GIT_COMMITTER_EMAIL", "t@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Build a git workspace where each entry in `commits` is committed in order,
/// then run the structural index (which also mines git temporal data).
pub fn index_git_fixture(commits: &[&[(&str, &str)]]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    for files in commits {
        write_files(dir.path(), files);
        git(dir.path(), &["add", "-A"]);
        git(
            dir.path(),
            &["commit", "-q", "--no-gpg-sign", "-m", "change"],
        );
    }
    index_dir(dir)
}

#[derive(Debug)]
pub struct RefInfo {
    pub source_path: String,
    pub source_symbol: Option<String>,
    pub confidence: f64,
    pub target_path: Option<String>,
    pub kind: String,
}

/// All non-import refs pointing at `target_name`, with resolved target paths.
pub fn refs_to(repo: &Repository, target_name: &str) -> Vec<RefInfo> {
    let conn = repo.conn();
    let mut stmt = conn
        .prepare(
            "SELECT sf.path, r.source_symbol, r.confidence, tf.path, r.kind
             FROM refs r
             JOIN files sf ON r.source_file_id = sf.id
             LEFT JOIN files tf ON r.target_file_id = tf.id
             WHERE r.target_name = ?1 AND r.kind != 'import'
             ORDER BY r.confidence DESC",
        )
        .unwrap();
    let rows = stmt
        .query_map([target_name], |row| {
            Ok(RefInfo {
                source_path: row.get(0)?,
                source_symbol: row.get(1)?,
                confidence: row.get(2)?,
                target_path: row.get(3)?,
                kind: row.get(4)?,
            })
        })
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

/// Only call/field_access refs (what the caller-tracing queries consume).
pub fn call_refs_to(repo: &Repository, target_name: &str) -> Vec<RefInfo> {
    refs_to(repo, target_name)
        .into_iter()
        .filter(|r| r.kind == "call" || r.kind == "field_access")
        .collect()
}

/// Filter a refs slice to those whose `source_path` matches — the most common
/// trim done in resolver/MRO tests before asserting on a single ref.
pub fn refs_from<'a>(refs: &'a [RefInfo], source_path: &str) -> Vec<&'a RefInfo> {
    refs.iter()
        .filter(|r| r.source_path == source_path)
        .collect()
}

/// Assert that `refs` contains exactly one ref with `confidence == expected_conf`
/// resolved to `target_path`. Panics on length mismatch (with the full ref list
/// for debugging) or on confidence/path mismatch. Use this for the common
/// resolver tier shape: `len == 1 && confidence == X && target == Y`.
pub fn assert_unique_resolved(refs: &[RefInfo], expected_conf: f64, target_path: &str) {
    assert_eq!(
        refs.len(),
        1,
        "expected exactly one ref resolved to {target_path} at {expected_conf}, got: {refs:?}"
    );
    assert_eq!(
        refs[0].confidence, expected_conf,
        "confidence mismatch (target_path={target_path}): {:?}",
        refs[0]
    );
    assert_eq!(
        refs[0].target_path.as_deref(),
        Some(target_path),
        "target path mismatch: {:?}",
        refs[0]
    );
}

/// Same as [`assert_unique_resolved`] but also pins `source_symbol` — used by
/// tests that care which enclosing function owns the call edge.
pub fn assert_unique_resolved_at(
    refs: &[RefInfo],
    expected_conf: f64,
    target_path: &str,
    source_symbol: &str,
) {
    assert_unique_resolved(refs, expected_conf, target_path);
    assert_eq!(
        refs[0].source_symbol.as_deref(),
        Some(source_symbol),
        "source_symbol mismatch: {:?}",
        refs[0]
    );
}

/// Reference-slice variant of [`assert_unique_resolved`] — accepts the borrowed
/// slice returned by [`refs_from`] without forcing the caller to clone.
pub fn assert_unique_resolved_borrowed(refs: &[&RefInfo], expected_conf: f64, target_path: &str) {
    assert_eq!(
        refs.len(),
        1,
        "expected exactly one ref resolved to {target_path} at {expected_conf}, got: {refs:?}"
    );
    assert_eq!(
        refs[0].confidence, expected_conf,
        "confidence mismatch (target_path={target_path}): {:?}",
        refs[0]
    );
    assert_eq!(
        refs[0].target_path.as_deref(),
        Some(target_path),
        "target path mismatch: {:?}",
        refs[0]
    );
}
