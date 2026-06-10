use anyhow::Result;
use rusqlite::params;

use crate::storage::Repository;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolResult {
    pub chunk_id: i64,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line: u32,
    pub scope: Option<String>,
    pub score: f64,
}

pub fn search(repo: &Repository, query: &str, limit: usize) -> Result<Vec<SymbolResult>> {
    let conn = repo.conn();

    // Exact match first, then prefix, then contains
    let mut results = Vec::new();

    // Exact match (highest score)
    let mut stmt = conn.prepare(
        "SELECT s.chunk_id, s.name, s.kind, f.path, s.line, s.scope
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.name = ?1 LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![query, limit as i64], |row| {
        Ok(SymbolResult {
            chunk_id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            file_path: row.get(3)?,
            line: row.get(4)?,
            scope: row.get(5)?,
            score: 1.0,
        })
    })?;
    results.extend(rows.filter_map(|r| r.ok()));

    if results.len() >= limit {
        results.truncate(limit);
        return Ok(results);
    }

    // Prefix match
    let pattern = format!("{query}%");
    let mut stmt = conn.prepare(
        "SELECT s.chunk_id, s.name, s.kind, f.path, s.line, s.scope
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.name LIKE ?1 AND s.name != ?2 LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![pattern, query, (limit - results.len()) as i64],
        |row| {
            Ok(SymbolResult {
                chunk_id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
                line: row.get(4)?,
                scope: row.get(5)?,
                score: 0.8,
            })
        },
    )?;
    results.extend(rows.filter_map(|r| r.ok()));

    if results.len() >= limit {
        results.truncate(limit);
        return Ok(results);
    }

    // Contains match
    let pattern = format!("%{query}%");
    let mut stmt = conn.prepare(
        "SELECT s.chunk_id, s.name, s.kind, f.path, s.line, s.scope
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.name LIKE ?1 AND s.name NOT LIKE ?2 LIMIT ?3",
    )?;
    let prefix_pattern = format!("{query}%");
    let rows = stmt.query_map(
        params![pattern, prefix_pattern, (limit - results.len()) as i64],
        |row| {
            Ok(SymbolResult {
                chunk_id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                file_path: row.get(3)?,
                line: row.get(4)?,
                scope: row.get(5)?,
                score: 0.5,
            })
        },
    )?;
    results.extend(rows.filter_map(|r| r.ok()));

    results.truncate(limit);
    Ok(results)
}
