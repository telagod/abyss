use anyhow::Result;
use rusqlite::params;
use tracing::info;

use crate::storage::Repository;

pub fn compute_file_metrics(repo: &Repository, days_30: u32, days_90: u32) -> Result<u64> {
    let conn = repo.conn();

    // Update git metrics on existing file_metrics rows (preserve cyclomatic/max_func_lines)
    conn.execute(
        "UPDATE file_metrics SET
            change_count_30d = COALESCE((SELECT COUNT(DISTINCT cf.commit_id) FROM commit_files cf
                JOIN commits c ON cf.commit_id = c.id JOIN files f ON f.path = cf.file_path
                WHERE f.id = file_metrics.file_id AND c.ts > unixepoch() - ?1 * 86400), 0),
            change_count_90d = COALESCE((SELECT COUNT(DISTINCT cf.commit_id) FROM commit_files cf
                JOIN commits c ON cf.commit_id = c.id JOIN files f ON f.path = cf.file_path
                WHERE f.id = file_metrics.file_id AND c.ts > unixepoch() - ?2 * 86400), 0),
            last_changed_ts = (SELECT MAX(c.ts) FROM commit_files cf
                JOIN commits c ON cf.commit_id = c.id JOIN files f ON f.path = cf.file_path
                WHERE f.id = file_metrics.file_id),
            unique_authors = COALESCE((SELECT COUNT(DISTINCT c.author) FROM commit_files cf
                JOIN commits c ON cf.commit_id = c.id JOIN files f ON f.path = cf.file_path
                WHERE f.id = file_metrics.file_id), 0),
            has_tests = (SELECT CASE WHEN f.path LIKE '%_test.%' OR f.path LIKE '%/test/%'
                OR f.path LIKE '%.test.%' OR f.path LIKE '%.spec.%' THEN 1 ELSE 0 END
                FROM files f WHERE f.id = file_metrics.file_id),
            hotspot_score = 0",
        params![days_30, days_90],
    )?;

    // Second pass: compute hotspot_score using the updated values
    conn.execute(
        "UPDATE file_metrics SET hotspot_score = COALESCE(change_count_30d, 0) * COALESCE(cyclomatic, 1)
         WHERE change_count_30d > 0",
        [],
    )?;

    let updated: i64 = conn.query_row("SELECT COUNT(*) FROM file_metrics", [], |r| r.get(0))?;
    info!("computed metrics for {} files", updated);
    Ok(updated as u64)
}

pub fn update_complexity(
    repo: &Repository,
    file_id: i64,
    cyclomatic: f64,
    max_func_lines: u32,
) -> Result<()> {
    repo.conn().execute(
        "UPDATE file_metrics SET cyclomatic = ?1, max_func_lines = ?2,
            hotspot_score = change_count_30d * ?1
         WHERE file_id = ?3",
        params![cyclomatic, max_func_lines, file_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HotspotItem {
    pub file_path: String,
    pub change_count: u32,
    pub complexity: f64,
    pub hotspot_score: f64,
    pub max_func_lines: u32,
    pub unique_authors: u32,
}

pub fn top_hotspots(repo: &Repository, limit: usize) -> Result<Vec<HotspotItem>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare(
        "SELECT f.path, fm.change_count_30d, fm.cyclomatic, fm.hotspot_score,
                fm.max_func_lines, fm.unique_authors
         FROM file_metrics fm JOIN files f ON fm.file_id = f.id
         WHERE fm.hotspot_score > 0
         ORDER BY fm.hotspot_score DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(HotspotItem {
            file_path: row.get(0)?,
            change_count: row.get(1)?,
            complexity: row.get(2)?,
            hotspot_score: row.get(3)?,
            max_func_lines: row.get(4)?,
            unique_authors: row.get(5)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
