//! Token savings tracking — records per-command compression stats.
//!
//! Reuses the existing `.code-abyss/index.db` to avoid a second DB file.
//! Table: `proxy_tracking`.

use anyhow::Result;
use rusqlite::Connection;

use super::estimate_tokens;

pub fn ensure_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS proxy_tracking (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL DEFAULT (datetime('now')),
            command TEXT NOT NULL,
            raw_tokens INTEGER NOT NULL,
            filtered_tokens INTEGER NOT NULL,
            savings_pct REAL NOT NULL,
            exec_ms INTEGER NOT NULL,
            project TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_proxy_tracking_ts
            ON proxy_tracking(timestamp);",
    )?;
    Ok(())
}

pub fn record(
    conn: &Connection,
    command: &str,
    raw: &str,
    filtered: &str,
    exec_ms: u64,
    project: Option<&str>,
) -> Result<()> {
    let raw_tokens = estimate_tokens(raw) as i64;
    let filtered_tokens = estimate_tokens(filtered) as i64;
    let savings_pct = if raw_tokens > 0 {
        ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64) * 100.0
    } else {
        0.0
    };

    conn.execute(
        "INSERT INTO proxy_tracking (command, raw_tokens, filtered_tokens, savings_pct, exec_ms, project)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![command, raw_tokens, filtered_tokens, savings_pct, exec_ms as i64, project],
    )?;
    Ok(())
}

/// Summary stats for `abyss gain`.
#[derive(Debug, Default)]
pub struct GainSummary {
    pub total_commands: u64,
    pub total_raw_tokens: u64,
    pub total_filtered_tokens: u64,
    pub total_saved_tokens: u64,
    pub avg_savings_pct: f64,
    pub top_commands: Vec<(String, u64, f64)>,
}

pub fn gain_summary(conn: &Connection, days: u32) -> Result<GainSummary> {
    ensure_table(conn)?;

    let mut summary = GainSummary::default();

    let row = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(raw_tokens), 0), COALESCE(SUM(filtered_tokens), 0)
         FROM proxy_tracking
         WHERE timestamp >= datetime('now', ?1)",
        [format!("-{days} days")],
        |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?, r.get::<_, u64>(2)?)),
    )?;

    summary.total_commands = row.0;
    summary.total_raw_tokens = row.1;
    summary.total_filtered_tokens = row.2;
    summary.total_saved_tokens = row.1.saturating_sub(row.2);

    if summary.total_raw_tokens > 0 {
        summary.avg_savings_pct =
            (summary.total_saved_tokens as f64 / summary.total_raw_tokens as f64) * 100.0;
    }

    // top commands by total savings
    let mut stmt = conn.prepare(
        "SELECT command,
                SUM(raw_tokens - filtered_tokens) as saved,
                AVG(savings_pct) as avg_pct
         FROM proxy_tracking
         WHERE timestamp >= datetime('now', ?1)
         GROUP BY command
         ORDER BY saved DESC
         LIMIT 10",
    )?;
    let rows = stmt.query_map([format!("-{days} days")], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, f64>(2)?))
    })?;
    summary.top_commands = rows.flatten().collect();

    Ok(summary)
}

pub fn render_gain(summary: &GainSummary) -> String {
    if summary.total_commands == 0 {
        return "No proxy data yet. Run commands through `abyss proxy` to start tracking.".into();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Token savings ({} commands)\n",
        summary.total_commands
    ));
    out.push_str(&format!(
        "  Raw:      {:>8} tokens\n",
        summary.total_raw_tokens
    ));
    out.push_str(&format!(
        "  Filtered: {:>8} tokens\n",
        summary.total_filtered_tokens
    ));
    out.push_str(&format!(
        "  Saved:    {:>8} tokens ({:.1}%)\n",
        summary.total_saved_tokens, summary.avg_savings_pct
    ));

    if !summary.top_commands.is_empty() {
        out.push_str("\nTop commands by savings:\n");
        for (cmd, saved, pct) in &summary.top_commands {
            let bar_len = (*pct / 5.0).round() as usize;
            let bar: String = "█".repeat(bar_len.min(20));
            out.push_str(&format!("  {cmd:<30} {saved:>6} saved ({pct:.0}%) {bar}\n"));
        }
    }
    out
}
