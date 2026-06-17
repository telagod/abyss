//! Search ranking guarantees: real implementations should out-rank their
//! test/import-mention siblings.
//!
//! Synthetic fixture (skips the indexer entirely so we get deterministic
//! centrality numbers): three chunks named "middleware" — one is the impl
//! with high centrality, one is its test file, one is an import line. Order
//! after `search("middleware")` must be [impl, test, import].

use code_abyss::config::Config;
use code_abyss::search::fulltext;
use code_abyss::storage::Repository;
use tempfile::TempDir;

fn open_repo(dir: &TempDir) -> Repository {
    let ws = dir.path();
    let config = Config::new(ws);
    Repository::open(&config.db_path, config.model.dimensions).unwrap()
}

fn seed_file(repo: &Repository, path: &str, centrality: f64) -> i64 {
    // upsert_file uses the file's path's extension to derive language; .ts
    // works for everything below.
    let fid = repo
        .upsert_file(path, "deadbeef", Some("typescript"), 0, 0, false)
        .unwrap();
    repo.conn()
        .execute(
            "INSERT INTO arch_facts(file_id, centrality) VALUES (?1, ?2)
             ON CONFLICT(file_id) DO UPDATE SET centrality = ?2",
            rusqlite::params![fid, centrality],
        )
        .unwrap();
    fid
}

fn seed_chunk(repo: &Repository, file_id: i64, kind: &str, content: &str) -> i64 {
    repo.insert_chunk(file_id, content, kind, 1, 10, None, 50)
        .unwrap()
}

#[test]
fn impl_outranks_test_and_import() {
    let dir = tempfile::tempdir().unwrap();
    let repo = open_repo(&dir);

    // impl: high centrality, class chunk.
    let impl_fid = seed_file(&repo, "src/middleware/auth.ts", 0.5);
    let impl_cid = seed_chunk(
        &repo,
        impl_fid,
        "class",
        "export class AuthMiddleware {\n  // middleware that authenticates\n  apply() { /* middleware logic */ }\n}",
    );

    // test: low centrality, generic block chunk, .test.ts path.
    let test_fid = seed_file(&repo, "src/middleware/auth.test.ts", 0.1);
    let test_cid = seed_chunk(
        &repo,
        test_fid,
        "block",
        "describe('middleware', () => {\n  it('runs middleware', () => { /* middleware test */ });\n});",
    );

    // import: low centrality, kind=import.
    let imp_fid = seed_file(&repo, "src/utils/helpers.ts", 0.05);
    let imp_cid = seed_chunk(
        &repo,
        imp_fid,
        "import",
        "import { middleware } from '../middleware/auth';",
    );

    let results = fulltext::search(&repo, "middleware", 10).unwrap();
    assert!(
        results.len() >= 3,
        "expected all three chunks to match, got {}",
        results.len()
    );

    let order: Vec<i64> = results.iter().map(|r| r.chunk_id).collect();
    let pos = |cid: i64| order.iter().position(|c| *c == cid).expect("chunk missing");
    let p_impl = pos(impl_cid);
    let p_test = pos(test_cid);
    let p_imp = pos(imp_cid);

    assert!(
        p_impl < p_test,
        "impl chunk (pos {p_impl}) should rank above test chunk (pos {p_test}); order={order:?}"
    );
    assert!(
        p_test < p_imp,
        "test chunk (pos {p_test}) should rank above import chunk (pos {p_imp}); order={order:?}"
    );
}

#[test]
fn centrality_boost_separates_otherwise_equal_chunks() {
    // Two identical chunks differing only by file centrality — the high
    // centrality one must win.
    let dir = tempfile::tempdir().unwrap();
    let repo = open_repo(&dir);

    let hi_fid = seed_file(&repo, "src/core/router.ts", 0.6);
    let hi_cid = seed_chunk(&repo, hi_fid, "class", "function router() { /* router */ }");

    let lo_fid = seed_file(&repo, "src/util/router.ts", 0.0);
    let lo_cid = seed_chunk(&repo, lo_fid, "class", "function router() { /* router */ }");

    let results = fulltext::search(&repo, "router", 10).unwrap();
    let order: Vec<i64> = results.iter().map(|r| r.chunk_id).collect();
    let p_hi = order.iter().position(|c| *c == hi_cid).unwrap();
    let p_lo = order.iter().position(|c| *c == lo_cid).unwrap();
    assert!(
        p_hi < p_lo,
        "high centrality chunk should outrank zero centrality; order={order:?}"
    );
}

#[test]
fn missing_arch_facts_row_does_not_break_search() {
    // Files without an arch_facts row (e.g. before the L0 pass ran) must still
    // come back, just with no centrality boost.
    let dir = tempfile::tempdir().unwrap();
    let repo = open_repo(&dir);

    let fid = repo
        .upsert_file("src/a.ts", "h", Some("typescript"), 0, 0, false)
        .unwrap();
    // No arch_facts insert at all.
    let cid = seed_chunk(&repo, fid, "function", "function widget() { /* widget */ }");

    let results = fulltext::search(&repo, "widget", 10).unwrap();
    assert_eq!(results.len(), 1, "expected 1 hit, got {}", results.len());
    assert_eq!(results[0].chunk_id, cid);
}
