use anyhow::Result;

use crate::embedding::Embedder;
use crate::storage::Repository;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SemanticResult {
    pub chunk_id: i64,
    pub score: f64,
}

pub fn search(
    repo: &Repository,
    embedder: &Embedder,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticResult>> {
    let query_embedding = embedder.embed(query)?;
    let results = repo.search_vectors(&query_embedding, limit)?;

    Ok(results
        .into_iter()
        .map(|(chunk_id, distance)| SemanticResult {
            chunk_id,
            score: 1.0 / (1.0 + distance), // convert distance to similarity
        })
        .collect())
}
