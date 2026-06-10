use std::path::Path;
use std::process::Command;

use anyhow::Result;
use serde::Serialize;

use crate::storage::Repository;

#[derive(Debug, Clone, Serialize)]
pub struct EvolutionTrace {
    pub file_path: String,
    pub symbol: Option<String>,
    pub commits: Vec<EvolutionCommit>,
    pub coupled_files: Vec<CoupledFile>,
    pub churn_rate: f64,
    pub unique_authors: u32,
    pub total_changes: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvolutionCommit {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoupledFile {
    pub path: String,
    pub co_changes: u32,
    pub coupling_score: f64,
}

pub fn trace_evolution(
    workspace: &Path,
    repo: &Repository,
    file_path: &str,
    symbol: Option<&str>,
) -> Result<EvolutionTrace> {
    // Get commits for this file/symbol
    let commits = if let Some(sym) = symbol {
        git_log_function(workspace, file_path, sym)?
    } else {
        git_log_file(workspace, file_path)?
    };

    // Get coupled files from DB
    let conn = repo.conn();
    let mut stmt = conn.prepare(
        "SELECT file_b, co_changes, coupling_score FROM change_coupling WHERE file_a = ?1
         UNION ALL
         SELECT file_a, co_changes, coupling_score FROM change_coupling WHERE file_b = ?1
         ORDER BY coupling_score DESC LIMIT 10")?;
    let coupled: Vec<CoupledFile> = stmt.query_map([file_path], |row| {
        Ok(CoupledFile {
            path: row.get(0)?,
            co_changes: row.get(1)?,
            coupling_score: row.get(2)?,
        })
    })?.filter_map(|r| r.ok()).collect();

    // Get churn stats from DB
    let (total_changes, unique_authors): (u32, u32) = conn.query_row(
        "SELECT COALESCE(COUNT(DISTINCT cf.commit_id), 0), COALESCE(COUNT(DISTINCT c.author), 0)
         FROM commit_files cf JOIN commits c ON cf.commit_id = c.id
         WHERE cf.file_path = ?1",
        [file_path],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap_or((0, 0));

    // Churn rate: total lines changed / file size
    let churn_lines: i64 = conn.query_row(
        "SELECT COALESCE(SUM(added + deleted), 0) FROM commit_files WHERE file_path = ?1",
        [file_path], |r| r.get(0),
    ).unwrap_or(0);
    let file_size: i64 = conn.query_row(
        "SELECT COALESCE(size, 1) FROM files WHERE path = ?1", [file_path], |r| r.get(0),
    ).unwrap_or(1);
    let churn_rate = churn_lines as f64 / file_size.max(1) as f64;

    Ok(EvolutionTrace {
        file_path: file_path.to_string(),
        symbol: symbol.map(|s| s.to_string()),
        commits,
        coupled_files: coupled,
        churn_rate,
        unique_authors,
        total_changes,
    })
}

fn git_log_file(workspace: &Path, file_path: &str) -> Result<Vec<EvolutionCommit>> {
    let output = Command::new("git")
        .args(["log", "--format=%H|%an|%ai|%s", "-30", "--", file_path])
        .current_dir(workspace)
        .output()?;
    parse_log_output(&output.stdout)
}

fn git_log_function(workspace: &Path, file_path: &str, symbol: &str) -> Result<Vec<EvolutionCommit>> {
    // Try git log -L :symbol:file first
    let output = Command::new("git")
        .args(["log", "--format=%H|%an|%ai|%s", "--no-patch", "-20",
               &format!("-L:{}:{}", symbol, file_path)])
        .current_dir(workspace)
        .output()?;

    if output.status.success() {
        let commits = parse_log_output(&output.stdout)?;
        if !commits.is_empty() {
            return Ok(commits);
        }
    }

    // Fallback: search commits touching this file that mention the symbol
    let output = Command::new("git")
        .args(["log", "--format=%H|%an|%ai|%s", "-S", symbol, "-20", "--", file_path])
        .current_dir(workspace)
        .output()?;
    parse_log_output(&output.stdout)
}

fn parse_log_output(stdout: &[u8]) -> Result<Vec<EvolutionCommit>> {
    let text = String::from_utf8_lossy(stdout);
    let mut commits = Vec::new();
    for line in text.lines() {
        if line.is_empty() { continue; }
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() >= 4 {
            commits.push(EvolutionCommit {
                hash: parts[0][..8.min(parts[0].len())].to_string(),
                author: parts[1].to_string(),
                date: parts[2].split_whitespace().next().unwrap_or("").to_string(),
                message: parts[3].to_string(),
            });
        }
    }
    Ok(commits)
}
