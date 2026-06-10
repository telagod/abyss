use anyhow::Result;
use rusqlite::params;

use crate::storage::Repository;

#[derive(Debug, Clone, serde::Serialize)]
pub struct FulltextResult {
    pub chunk_id: i64,
    pub score: f64,
    pub snippet: String,
}

pub fn search(repo: &Repository, query: &str, limit: usize) -> Result<Vec<FulltextResult>> {
    let conn = repo.conn();
    let fts_query = build_fts_query(query);

    let mut stmt = conn.prepare(
        "SELECT rowid, rank, snippet(chunks_fts, 0, '>>>', '<<<', '...', 64)
         FROM chunks_fts
         WHERE chunks_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
        let rank: f64 = row.get(1)?;
        Ok(FulltextResult {
            chunk_id: row.get(0)?,
            score: -rank,
            snippet: row.get(2)?,
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Build FTS5 query from user input.
/// unicode61 tokenizer keeps CJK character runs as single tokens (e.g. "策略" is one token).
/// So we split on whitespace only, and use prefix match on the last term.
fn build_fts_query(query: &str) -> String {
    let terms: Vec<&str> = query.split_whitespace().filter(|t| !t.is_empty()).collect();

    if terms.is_empty() {
        return query.to_string();
    }

    if terms.len() == 1 {
        // Single term: prefix search (works for both CJK and ASCII)
        format!("{}*", escape_fts(terms[0]))
    } else {
        // Multiple terms: AND all, prefix on last
        let mut parts: Vec<String> = terms[..terms.len() - 1]
            .iter()
            .map(|t| escape_fts(t))
            .collect();
        parts.push(format!("{}*", escape_fts(terms.last().unwrap())));
        parts.join(" ")
    }
}

fn escape_fts(s: &str) -> String {
    // Wrap in quotes if contains special FTS chars
    if s.contains('"') || s.contains('*') || s.contains('(') || s.contains(')') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
