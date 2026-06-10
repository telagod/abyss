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

        let mut ranked: Vec<(i64, f64, Vec<String>)> = scores
            .into_iter()
            .map(|(id, (score, sources))| (id, score, sources))
            .collect();
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
}
