use std::path::Path;
use std::process::Command;

use anyhow::Result;
use rusqlite::params;
use tracing::info;

use crate::storage::Repository;

pub struct GitStats {
    pub commits_parsed: u64,
    pub files_touched: u64,
}

pub struct GitData {
    pub commits: Vec<CommitRecord>,
    pub file_changes: Vec<(usize, FileChangeRecord)>, // (commit_index, record)
}

pub struct CommitRecord {
    pub hash: String,
    pub author: String,
    pub ts: i64,
    pub message: String,
}

pub struct FileChangeRecord {
    pub file_path: String,
    pub added: i64,
    pub deleted: i64,
}

/// Parse git log into memory (no DB access — safe for background thread)
pub fn parse_git_log_to_memory(workspace: &Path, since_days: u32) -> Result<GitData> {
    let since = format!("--since={since_days}.days.ago");
    let output = Command::new("git")
        .args(["log", "--numstat", "--format=COMMIT|%H|%an|%at|%s", &since])
        .current_dir(workspace)
        .output()?;

    if !output.status.success() {
        // Temporal data is best-effort: not a git repo, empty repo, locale-specific
        // error text, missing git — none of these should fail indexing.
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            "git log unavailable, skipping temporal data: {}",
            stderr.trim()
        );
        return Ok(GitData {
            commits: Vec::new(),
            file_changes: Vec::new(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut commits = Vec::new();
    let mut file_changes = Vec::new();
    let mut current_idx: Option<usize> = None;

    for line in stdout.lines() {
        if line.starts_with("COMMIT|") {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() >= 4 {
                commits.push(CommitRecord {
                    hash: parts[1].to_string(),
                    author: parts[2].to_string(),
                    ts: parts[3].parse().unwrap_or(0),
                    message: parts.get(4).unwrap_or(&"").to_string(),
                });
                current_idx = Some(commits.len() - 1);
            }
        } else if let Some(idx) = current_idx {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() == 3 && parts[0] != "-" {
                file_changes.push((
                    idx,
                    FileChangeRecord {
                        file_path: parts[2].to_string(),
                        added: parts[0].parse().unwrap_or(0),
                        deleted: parts[1].parse().unwrap_or(0),
                    },
                ));
            }
        }
    }

    Ok(GitData {
        commits,
        file_changes,
    })
}

/// Write pre-parsed git data into DB (must run on main thread)
pub fn write_git_data(repo: &Repository, data: &GitData) -> Result<GitStats> {
    let conn = repo.conn();
    let mut commits_parsed = 0u64;
    let mut files_touched = 0u64;

    conn.execute_batch("BEGIN TRANSACTION")?;

    let mut commit_ids: Vec<Option<i64>> = Vec::with_capacity(data.commits.len());

    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO commits(hash,author,ts,message) VALUES(?1,?2,?3,?4)",
        )?;
        for c in &data.commits {
            let changes = stmt.execute(params![&c.hash, &c.author, c.ts, &c.message])?;
            if changes > 0 {
                commit_ids.push(Some(conn.last_insert_rowid()));
                commits_parsed += 1;
            } else {
                // Already exists
                let id: Option<i64> = conn
                    .query_row("SELECT id FROM commits WHERE hash = ?1", [&c.hash], |r| {
                        r.get(0)
                    })
                    .ok();
                commit_ids.push(id);
            }
        }
    }

    {
        let mut stmt = conn.prepare_cached(
            "INSERT OR IGNORE INTO commit_files(commit_id,file_path,added,deleted) VALUES(?1,?2,?3,?4)")?;
        for (idx, fc) in &data.file_changes {
            if let Some(Some(commit_id)) = commit_ids.get(*idx) {
                stmt.execute(params![commit_id, &fc.file_path, fc.added, fc.deleted])?;
                files_touched += 1;
            }
        }
    }

    conn.execute_batch("COMMIT")?;

    info!(
        "git: {} new commits, {} file changes",
        commits_parsed, files_touched
    );
    Ok(GitStats {
        commits_parsed,
        files_touched,
    })
}

/// Legacy API — parse and write in one call
pub fn parse_git_log(workspace: &Path, repo: &Repository, since_days: u32) -> Result<GitStats> {
    let data = parse_git_log_to_memory(workspace, since_days)?;
    write_git_data(repo, &data)
}
