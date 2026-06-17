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
