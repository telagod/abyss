use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use super::{fulltext, semantic, symbol};
use crate::embedding::Embedder;
use crate::storage::Repository;

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub content: String,
    pub kind: String,
    pub scope: Option<String>,
    pub score: f64,
    pub match_sources: Vec<String>,
}

pub struct SearchEngine<'a> {
    repo: &'a Repository,
    embedder: Option<&'a Embedder>,
}

impl<'a> SearchEngine<'a> {
    pub fn new(repo: &'a Repository, embedder: Option<&'a Embedder>) -> Self {
        Self { repo, embedder }
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let fetch_limit = limit * 3;
        let k = 60.0;
        let mut scores: HashMap<i64, (f64, Vec<String>)> = HashMap::new();

        // Semantic search (only if embedder available and vectors exist)
        if let Some(embedder) = self.embedder
            && self.repo.has_vectors().unwrap_or(false)
            && let Ok(results) = semantic::search(self.repo, embedder, query, fetch_limit)
        {
            for (rank, r) in results.iter().enumerate() {
                let rrf = 1.0 / (k + rank as f64 + 1.0);
                let entry = scores.entry(r.chunk_id).or_insert((0.0, Vec::new()));
                entry.0 += rrf * 1.0;
                entry.1.push("semantic".to_string());
            }
        }

        // Symbol search
        if let Ok(results) = symbol::search(self.repo, query, fetch_limit) {
            for (rank, r) in results.iter().enumerate() {
                let rrf = 1.0 / (k + rank as f64 + 1.0);
                let entry = scores.entry(r.chunk_id).or_insert((0.0, Vec::new()));
                entry.0 += rrf * 1.2;
                entry.1.push("symbol".to_string());
            }
        }

        // Full-text search
        if let Ok(results) = fulltext::search(self.repo, query, fetch_limit) {
            for (rank, r) in results.iter().enumerate() {
                let rrf = 1.0 / (k + rank as f64 + 1.0);
                let entry = scores.entry(r.chunk_id).or_insert((0.0, Vec::new()));
                entry.0 += rrf * 0.8;
                entry.1.push("fulltext".to_string());
            }
        }

        // RRF sums "where each engine ranked this chunk" — but fulltext's
        // careful test/import demotion gets erased because semantic+symbol
        // engines vote test files just as enthusiastically (mocks named
        // `middleware1` in test files inflate symbol search for "middleware").
        // Re-apply file-level penalty AFTER fusion so the centrality+test
        // signals survive the RRF wash. Tuned on the hono dogfood session.
        let mut ranked: Vec<(i64, f64, Vec<String>)> = scores
            .into_iter()
            .map(|(id, (score, sources))| (id, score, sources))
            .collect();
        self.apply_file_penalty(&mut ranked)?;
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        ranked.truncate(limit);

        let mut results = Vec::new();
        for (chunk_id, score, sources) in ranked {
            if let Some(chunk) = self.repo.get_chunk(chunk_id)? {
                let file_path = self.repo.get_file_path(chunk.file_id)?.unwrap_or_default();
                results.push(SearchResult {
                    file_path,
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    content: chunk.content,
                    kind: chunk.kind,
                    scope: chunk.scope,
                    score,
                    match_sources: sources,
                });
            }
        }

        Ok(results)
    }

    /// Apply file-level test/import/centrality penalty to fused RRF scores.
    /// Same multipliers as fulltext.rs uses internally — applied post-fusion
    /// so symbol-search and semantic-search votes for test files don't drag
    /// noise to the top.
    fn apply_file_penalty(&self, ranked: &mut [(i64, f64, Vec<String>)]) -> Result<()> {
        if ranked.is_empty() {
            return Ok(());
        }
        let conn = self.repo.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, f.path, c.kind, COALESCE(af.centrality, 0)
             FROM chunks c
             JOIN files f ON f.id = c.file_id
             LEFT JOIN arch_facts af ON af.file_id = c.file_id
             WHERE c.id = ?1",
        )?;
        for entry in ranked.iter_mut() {
            let (chunk_id, score, _) = entry;
            let (path, kind, cent): (String, String, f64) = match stmt
                .query_row(rusqlite::params![*chunk_id], |row| {
                    Ok((row.get(1)?, row.get(2)?, row.get(3)?))
                }) {
                Ok(t) => t,
                Err(_) => continue,
            };
            // Aggressive demotion: by the time we're here the test path
            // already won via RRF on symbol + semantic search. Soft 0.4
            // wasn't enough on hono — needs ~10x crushing for impl files
            // to rise above test mocks of the same name.
            let test_pen = if path.contains("_test.")
                || path.contains(".test.")
                || path.contains("/test/")
                || path.contains("/__tests__/")
                || path.contains("/tests/")
            {
                0.1
            } else {
                1.0
            };
            let import_pen = if kind == "import" { 0.1 } else { 1.0 };
            let cent_boost = 1.0 + 5.0 * cent;
            *score *= test_pen * import_pen * cent_boost;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// In-memory-ish real Repository on a tempdir. The fusion logic and the
    /// `apply_file_penalty` SQL both hit `repo.conn()`, so we exercise the same
    /// schema/triggers the production path uses (FTS5 is populated by the
    /// `chunks_ai` trigger on every `insert_chunk`).
    struct TestRepo {
        _dir: TempDir,
        repo: Repository,
    }

    fn repo() -> TestRepo {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("index.db");
        let repo = Repository::open(&db, 768).unwrap();
        TestRepo { _dir: dir, repo }
    }

    /// Insert a file + one chunk + (optionally) one symbol whose name matches
    /// the chunk content, returning the chunk_id. `kind` is the chunk kind
    /// (e.g. "function" or "import"). When `symbol_name` is Some, a symbol row
    /// with that exact name is added so `symbol::search` can hit it.
    fn add_chunk(
        repo: &Repository,
        path: &str,
        content: &str,
        kind: &str,
        symbol_name: Option<&str>,
    ) -> i64 {
        let file_id = repo
            .upsert_file(path, "hash", Some("rust"), 0, 0, false)
            .unwrap();
        let chunk_id = repo
            .insert_chunk(file_id, content, kind, 1, 10, None, 0)
            .unwrap();
        if let Some(name) = symbol_name {
            repo.insert_symbol(chunk_id, file_id, name, "function", 1, None)
                .unwrap();
        }
        chunk_id
    }

    /// Set centrality on a file (drives the `cent_boost` in apply_file_penalty).
    fn set_centrality(repo: &Repository, path: &str, centrality: f64) {
        let file_id = repo.get_file_id(path).unwrap().unwrap();
        repo.conn()
            .execute(
                "INSERT INTO arch_facts(file_id, centrality) VALUES (?1, ?2)
                 ON CONFLICT(file_id) DO UPDATE SET centrality = ?2",
                rusqlite::params![file_id, centrality],
            )
            .unwrap();
    }

    fn engine(repo: &Repository) -> SearchEngine<'_> {
        SearchEngine::new(repo, None)
    }

    // ───────────────────────── apply_file_penalty ─────────────────────────

    #[test]
    fn penalty_demotes_test_file_paths_tenfold() {
        let t = repo();
        let impl_id = add_chunk(&t.repo, "src/auth.rs", "fn login() {}", "function", None);
        let test_id = add_chunk(
            &t.repo,
            "src/auth_test.rs",
            "fn login_mock() {}",
            "function",
            None,
        );

        let mut ranked = vec![
            (impl_id, 1.0, vec!["symbol".to_string()]),
            (test_id, 1.0, vec!["symbol".to_string()]),
        ];
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();

        let impl_score = ranked.iter().find(|r| r.0 == impl_id).unwrap().1;
        let test_score = ranked.iter().find(|r| r.0 == test_id).unwrap().1;
        assert_eq!(impl_score, 1.0, "impl file untouched");
        assert!(
            (test_score - 0.1).abs() < 1e-9,
            "test file demoted 10x, got {test_score}"
        );
    }

    #[test]
    fn penalty_recognizes_all_test_path_shapes() {
        let t = repo();
        // Every branch of the test-path predicate in apply_file_penalty.
        let shapes = [
            "src/foo_test.go",     // _test.
            "src/foo.test.ts",     // .test.
            "src/test/Foo.java",   // /test/
            "web/__tests__/x.tsx", // /__tests__/
            "pkg/tests/y.rs",      // /tests/
        ];
        let mut ranked = Vec::new();
        for (i, p) in shapes.iter().enumerate() {
            let id = add_chunk(&t.repo, p, &format!("fn f{i}() {{}}"), "function", None);
            ranked.push((id, 1.0, vec!["symbol".to_string()]));
        }
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();

        for (id, score, _) in &ranked {
            assert!(
                (score - 0.1).abs() < 1e-9,
                "chunk {id} should be demoted to 0.1, got {score}"
            );
        }
    }

    #[test]
    fn penalty_demotes_import_chunks_tenfold() {
        let t = repo();
        let import_id = add_chunk(&t.repo, "src/lib.rs", "use crate::auth;", "import", None);
        let fn_id = add_chunk(&t.repo, "src/svc.rs", "fn handler() {}", "function", None);

        let mut ranked = vec![
            (import_id, 2.0, vec!["fulltext".to_string()]),
            (fn_id, 2.0, vec!["fulltext".to_string()]),
        ];
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();

        let import_score = ranked.iter().find(|r| r.0 == import_id).unwrap().1;
        let fn_score = ranked.iter().find(|r| r.0 == fn_id).unwrap().1;
        assert!(
            (import_score - 0.2).abs() < 1e-9,
            "import chunk demoted 10x (2.0 -> 0.2), got {import_score}"
        );
        assert_eq!(fn_score, 2.0, "function chunk untouched");
    }

    #[test]
    fn penalty_applies_centrality_boost() {
        let t = repo();
        let id = add_chunk(&t.repo, "src/core.rs", "fn run() {}", "function", None);
        set_centrality(&t.repo, "src/core.rs", 0.5);

        let mut ranked = vec![(id, 1.0, vec!["symbol".to_string()])];
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();

        // cent_boost = 1 + 5 * 0.5 = 3.5
        assert!(
            (ranked[0].1 - 3.5).abs() < 1e-9,
            "centrality 0.5 should yield 3.5x, got {}",
            ranked[0].1
        );
    }

    #[test]
    fn penalty_compounds_test_and_centrality() {
        let t = repo();
        // A test file that's also central: 0.1 (test) * (1 + 5*0.2) = 0.1 * 2.0
        let id = add_chunk(
            &t.repo,
            "src/auth_test.rs",
            "fn login_mock() {}",
            "function",
            None,
        );
        set_centrality(&t.repo, "src/auth_test.rs", 0.2);

        let mut ranked = vec![(id, 1.0, vec!["symbol".to_string()])];
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();

        assert!(
            (ranked[0].1 - 0.2).abs() < 1e-9,
            "0.1 * 2.0 = 0.2 expected, got {}",
            ranked[0].1
        );
    }

    #[test]
    fn penalty_skips_unknown_chunk_ids() {
        let t = repo();
        // chunk_id 999999 has no row; apply_file_penalty must `continue`,
        // leaving the score untouched rather than erroring.
        let mut ranked = vec![(999_999_i64, 7.0, vec!["symbol".to_string()])];
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();
        assert_eq!(ranked[0].1, 7.0, "missing chunk left untouched");
    }

    #[test]
    fn penalty_on_empty_slice_is_noop() {
        let t = repo();
        let mut ranked: Vec<(i64, f64, Vec<String>)> = Vec::new();
        engine(&t.repo).apply_file_penalty(&mut ranked).unwrap();
        assert!(ranked.is_empty());
    }

    // ───────────────────────── search() / RRF fusion ─────────────────────────

    #[test]
    fn search_empty_index_returns_nothing() {
        let t = repo();
        let out = engine(&t.repo).search("anything", 10).unwrap();
        assert!(out.is_empty(), "empty index yields no results");
    }

    #[test]
    fn search_dedups_chunk_hit_by_both_engines() {
        let t = repo();
        // One chunk whose symbol name AND content both contain the query, so it
        // is voted by symbol::search and fulltext::search. It must appear once,
        // with both sources recorded.
        add_chunk(
            &t.repo,
            "src/widget.rs",
            "fn widget() { /* widget impl */ }",
            "function",
            Some("widget"),
        );

        let out = engine(&t.repo).search("widget", 10).unwrap();
        assert_eq!(out.len(), 1, "single chunk must not be duplicated");
        let sources = &out[0].match_sources;
        assert!(
            sources.contains(&"symbol".to_string()),
            "symbol source recorded: {sources:?}"
        );
        assert!(
            sources.contains(&"fulltext".to_string()),
            "fulltext source recorded: {sources:?}"
        );
    }

    #[test]
    fn search_rrf_weights_symbol_above_fulltext() {
        let t = repo();
        // Two chunks both rank #1 in their respective engine for "alpha":
        //  - sym_only: matched ONLY by symbol search (weight 1.2)
        //  - txt_only: matched ONLY by fulltext search (weight 0.8)
        // RRF score at rank 0 = 1/(60+1) * weight, so symbol's chunk must
        // out-score fulltext's chunk purely on the per-engine weight.
        //
        // sym_only: symbol name "alpha" matches; content avoids the term so
        //   fulltext won't also vote it.
        add_chunk(
            &t.repo,
            "src/sym.rs",
            "fn renamed() { do_stuff(); }",
            "function",
            Some("alpha"),
        );
        // txt_only: content contains "alpha"; symbol name differs so symbol
        //   search won't vote it.
        add_chunk(
            &t.repo,
            "src/txt.rs",
            "fn helper() { /* alpha pathway */ }",
            "function",
            Some("zzz_other"),
        );

        let out = engine(&t.repo).search("alpha", 10).unwrap();
        assert_eq!(out.len(), 2, "both chunks should surface: {out:?}");

        let sym = out.iter().find(|r| r.file_path == "src/sym.rs").unwrap();
        let txt = out.iter().find(|r| r.file_path == "src/txt.rs").unwrap();
        assert_eq!(sym.match_sources, vec!["symbol".to_string()]);
        assert_eq!(txt.match_sources, vec!["fulltext".to_string()]);

        // Both at rank 0 → rrf base equal; ratio is purely the weight ratio.
        let rrf0 = 1.0 / (60.0 + 0.0 + 1.0);
        assert!(
            (sym.score - rrf0 * 1.2).abs() < 1e-9,
            "symbol-only score = rrf*1.2, got {}",
            sym.score
        );
        assert!(
            (txt.score - rrf0 * 0.8).abs() < 1e-9,
            "fulltext-only score = rrf*0.8, got {}",
            txt.score
        );
        // Ordering: highest score first.
        assert_eq!(
            out[0].file_path, "src/sym.rs",
            "symbol-weighted hit must rank first"
        );
        assert!(out[0].score > out[1].score, "results sorted descending");
    }

    #[test]
    fn search_accumulates_rrf_across_engines() {
        let t = repo();
        // A chunk hit by BOTH engines must score symbol+fulltext combined,
        // beating a chunk hit by fulltext alone — proves the entry.0 += rrf
        // accumulation rather than overwrite.
        add_chunk(
            &t.repo,
            "src/both.rs",
            "fn beta() { /* beta core */ }",
            "function",
            Some("beta"),
        );
        add_chunk(
            &t.repo,
            "src/single.rs",
            "fn other() { /* beta mention */ }",
            "function",
            Some("unrelated"),
        );

        let out = engine(&t.repo).search("beta", 10).unwrap();
        let both = out.iter().find(|r| r.file_path == "src/both.rs").unwrap();
        let single = out.iter().find(|r| r.file_path == "src/single.rs").unwrap();

        assert_eq!(both.match_sources.len(), 2, "both engines: {both:?}");
        assert_eq!(single.match_sources, vec!["fulltext".to_string()]);
        assert!(
            both.score > single.score,
            "dual-engine chunk ({}) must out-score single-engine ({})",
            both.score,
            single.score
        );
    }

    #[test]
    fn search_respects_limit() {
        let t = repo();
        for i in 0..5 {
            add_chunk(
                &t.repo,
                &format!("src/f{i}.rs"),
                &format!("fn gamma{i}() {{ /* gamma */ }}"),
                "function",
                Some(&format!("gamma{i}")),
            );
        }
        let out = engine(&t.repo).search("gamma", 2).unwrap();
        assert_eq!(out.len(), 2, "limit truncates fused results");
    }

    #[test]
    fn search_post_fusion_penalty_sinks_test_file() {
        let t = repo();
        // Both files hit the same query via symbol+fulltext (equal raw RRF),
        // but the test file gets crushed 10x post-fusion, so the impl wins.
        add_chunk(
            &t.repo,
            "src/handler.rs",
            "fn delta() { /* delta */ }",
            "function",
            Some("delta"),
        );
        add_chunk(
            &t.repo,
            "src/handler_test.rs",
            "fn delta() { /* delta */ }",
            "function",
            Some("delta"),
        );

        let out = engine(&t.repo).search("delta", 10).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].file_path, "src/handler.rs",
            "impl file must out-rank test file after penalty: {out:?}"
        );
        assert!(out[0].score > out[1].score);
    }
}
