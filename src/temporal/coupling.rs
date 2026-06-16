use anyhow::Result;
use rusqlite::params;
use tracing::info;

use crate::storage::Repository;

/// Commits touching more than this many indexed files are excluded from change
/// coupling. A `prettier --write .` / dep-bump / license-header commit couples
/// every file to every other (O(N²) pairs) — a signal that's both false (no
/// logical cohesion) and the source of runaway memory in the self-join.
const BULK_COMMIT_THRESHOLD: i64 = 50;

#[derive(Debug, Clone, serde::Serialize)]
pub struct CouplingPair {
    pub file_a: String,
    pub file_b: String,
    pub co_changes: u32,
    pub coupling_score: f64,
}

pub fn compute_change_coupling(repo: &Repository, min_co_changes: u32) -> Result<u64> {
    let conn = repo.conn();

    conn.execute("DELETE FROM change_coupling", [])?;

    conn.execute(
        "INSERT INTO change_coupling (file_a, file_b, co_changes, total_changes, coupling_score)
         SELECT
            a.file_path,
            b.file_path,
            COUNT(DISTINCT a.commit_id),
            (SELECT COUNT(DISTINCT commit_id) FROM commit_files WHERE file_path = a.file_path),
            CAST(COUNT(DISTINCT a.commit_id) AS REAL) /
                MAX(1, (SELECT COUNT(DISTINCT commit_id) FROM commit_files WHERE file_path = a.file_path))
         FROM commit_files a
         JOIN commit_files b ON a.commit_id = b.commit_id AND a.file_path < b.file_path
         WHERE a.commit_id IN (
            SELECT commit_id FROM commit_files GROUP BY commit_id
            HAVING COUNT(*) <= ?2)
         GROUP BY a.file_path, b.file_path
         HAVING COUNT(DISTINCT a.commit_id) >= ?1",
        params![min_co_changes, BULK_COMMIT_THRESHOLD],
    )?;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM change_coupling", [], |r| r.get(0))?;
    info!(
        "computed {} coupling pairs (min co-changes: {})",
        count, min_co_changes
    );
    Ok(count as u64)
}

pub fn top_coupled(repo: &Repository, limit: usize) -> Result<Vec<CouplingPair>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare(
        "SELECT file_a, file_b, co_changes, coupling_score
         FROM change_coupling
         ORDER BY coupling_score DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(CouplingPair {
            file_a: row.get(0)?,
            file_b: row.get(1)?,
            co_changes: row.get(2)?,
            coupling_score: row.get(3)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
