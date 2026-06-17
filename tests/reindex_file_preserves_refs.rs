//! Regression: a single-file re-index must NOT silently nuke that file's
//! outgoing call edges.
//!
//! Pre-fix, `reindex_file` did `repo.delete_file(id)` before re-inserting the
//! file's parse output. SQLite's `ON DELETE CASCADE` on `refs.source_file_id`
//! erased every edge originating from the file, and the per-file path never
//! re-ran the batch resolver — so the file's calls dropped to confidence=0.0
//! (or vanished entirely) until the next full pass. The watcher V1 fix routed
//! modifies through `run_structural`, but `reindex_file` itself stayed a
//! footgun.
//!
//! This test indexes a two-file fixture (file1 calls Target from file0),
//! records the call edge, edits file1 (no semantic change to the call), runs
//! `reindex_file`, and asserts the call edge is still there with its
//! pre-edit resolution.

mod common;

use code_abyss::indexer::IndexPipeline;
use common::{Fixture, index_fixture};

/// Returns (count, resolved_count) for refs whose target_name is `target` and
/// whose source file is `source_path`. resolved_count tracks how many landed
/// at confidence ≥ 0.7 (the agent-facing gate).
fn outgoing_call_count(fx: &Fixture, source_path: &str, target: &str) -> (usize, usize) {
    let conn = fx.repo.conn();
    let mut stmt = conn
        .prepare(
            "SELECT r.confidence
             FROM refs r
             JOIN files f ON r.source_file_id = f.id
             WHERE f.path = ?1 AND r.target_name = ?2 AND r.kind = 'call'",
        )
        .unwrap();
    let rows: Vec<f64> = stmt
        .query_map([source_path, target], |row| row.get::<_, f64>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    let resolved = rows.iter().filter(|c| **c >= 0.7).count();
    (rows.len(), resolved)
}

#[test]
fn reindex_file_preserves_outgoing_refs() {
    let fx = index_fixture(&[
        (
            "app/a.go",
            "package app\n\nfunc Target() int { return 1 }\n",
        ),
        (
            "app/b.go",
            "package app\n\nfunc DirectB() int { return Target() }\n",
        ),
    ]);

    // Baseline: file b.go has one resolved call to Target.
    let (pre_count, pre_resolved) = outgoing_call_count(&fx, "app/b.go", "Target");
    assert_eq!(pre_count, 1, "expected 1 outgoing call to Target pre-edit");
    assert_eq!(
        pre_resolved, 1,
        "pre-edit call should be resolved at confidence ≥ 0.7"
    );

    // Snapshot the pre-edit file hash so we can prove the reindex landed.
    let b_path = fx.config.workspace.join("app/b.go");
    let pre_hash: String = fx
        .repo
        .conn()
        .query_row(
            "SELECT hash FROM files WHERE path = 'app/b.go'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    // Touch file b.go: add a sibling function so blake3 hash changes AND the
    // chunk set changes — but DirectB's call to Target stays identical.
    let edited = "package app\n\nfunc DirectB() int { return Target() }\n\nfunc Sibling() int { return 2 }\n";
    std::fs::write(&b_path, edited).unwrap();

    // Hit the public single-file API the watcher uses. Slim builds pass
    // `None` for the embedder; the structural pass underneath is what
    // matters for this contract — the file's outgoing call must survive
    // and re-resolve.
    let pipeline = IndexPipeline::new(fx.config.clone());
    pipeline.reindex_file(&fx.repo, None, &b_path).unwrap();

    let (post_count, post_resolved) = outgoing_call_count(&fx, "app/b.go", "Target");
    assert_eq!(
        post_count, 1,
        "outgoing call to Target lost after reindex_file (CASCADE regression)"
    );
    assert_eq!(
        post_resolved, 1,
        "outgoing call to Target left unresolved after reindex_file (skipped batch_resolve_refs regression)"
    );

    // Verify the edit actually landed in the index (defends against the
    // "we successfully no-op'd" failure mode).
    let conn = fx.repo.conn();
    let post_hash: String = conn
        .query_row(
            "SELECT hash FROM files WHERE path = 'app/b.go'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(
        pre_hash, post_hash,
        "file hash unchanged — reindex_file silently no-op'd the edit"
    );
    let sibling_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols s
             JOIN files f ON s.file_id = f.id
             WHERE f.path = 'app/b.go' AND s.name = 'Sibling'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        sibling_present, 1,
        "new Sibling symbol missing — reindex_file failed to ingest the post-edit AST"
    );
}
