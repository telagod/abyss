//! v0.5.5: `[ignore].patterns` from `.code-abyss/arch.toml` must filter the
//! indexer's WALKER output, not just the arch-inference rendering pass.
//! Pre-fix, vendored / generated trees were parsed and indexed; users only
//! saw them disappear from the arch layer view. Now they never enter the
//! call graph or FTS index in the first place.

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;
use tempfile::TempDir;

fn write(root: &std::path::Path, rel: &str, content: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, content).unwrap();
}

fn indexed_paths(repo: &Repository) -> Vec<String> {
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT path FROM files ORDER BY path")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
}

#[test]
fn ignore_patterns_filter_walker_output() {
    // arch.toml's [ignore].patterns = ["^vendor/"] must remove the vendor
    // tree from the indexer walker entirely.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "src/main.go", "package main\nfunc Main() {}\n");
    write(root, "vendor/lib/x.go", "package lib\nfunc Vendored() {}\n");
    write(
        root,
        ".code-abyss/arch.toml",
        "[ignore]\npatterns = [\"^vendor/\"]\n",
    );

    let ws = std::fs::canonicalize(root).unwrap();
    let config = Config::new(&ws);
    let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();

    let paths = indexed_paths(&repo);
    assert!(
        paths.iter().any(|p| p == "src/main.go"),
        "src/main.go must be indexed, got {paths:?}",
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("vendor/")),
        "vendor/* must NOT be indexed (arch.toml [ignore].patterns), got {paths:?}",
    );
}

#[test]
fn no_arch_toml_means_no_filtering() {
    // Defensive regression: without arch.toml the pipeline runs as before.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "src/main.go", "package main\nfunc Main() {}\n");
    write(root, "vendor/lib/x.go", "package lib\nfunc Vendored() {}\n");
    // Note: the FileWalker still hard-codes `vendor` in its filter_entry
    // blocklist (alongside node_modules/.git/target), so this fixture
    // verifies the NO-arch.toml path through the new code — the
    // load_overrides None branch — by checking that src/main.go still
    // makes it in (the vendor/ tree is independently blocked by the
    // walker's built-in skiplist).
    let ws = std::fs::canonicalize(root).unwrap();
    let config = Config::new(&ws);
    let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();

    let paths = indexed_paths(&repo);
    assert!(
        paths.iter().any(|p| p == "src/main.go"),
        "src/main.go must be indexed with no arch.toml present, got {paths:?}",
    );
}

#[test]
fn ignore_patterns_can_target_paths_walker_would_not_skip() {
    // Real value of the wiring: filter paths the walker doesn't already
    // hard-code (vendor/node_modules/target). User can express "ignore my
    // generated stubs at proto/gen/" via arch.toml.
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "src/main.go", "package main\nfunc Main() {}\n");
    write(
        root,
        "proto/gen/api.pb.go",
        "package api\nfunc Generated() {}\n",
    );
    write(
        root,
        ".code-abyss/arch.toml",
        "[ignore]\npatterns = [\"^proto/gen/\"]\n",
    );

    let ws = std::fs::canonicalize(root).unwrap();
    let config = Config::new(&ws);
    let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();

    let paths = indexed_paths(&repo);
    assert!(
        paths.iter().any(|p| p == "src/main.go"),
        "src/main.go must be indexed, got {paths:?}",
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("proto/gen/")),
        "proto/gen/* must NOT be indexed (arch.toml [ignore].patterns), got {paths:?}",
    );
}
