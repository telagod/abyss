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

/// Returns true if the proxy_tracking table has any rows at all.
pub fn has_any_data(conn: &Connection) -> bool {
    ensure_table(conn).ok();
    conn.query_row("SELECT COUNT(*) FROM proxy_tracking", [], |r| {
        r.get::<_, u64>(0)
    })
    .unwrap_or(0)
        > 0
}

#[derive(Debug, Default)]
pub struct GainSummary {
    pub total_commands: u64,
    pub total_raw_tokens: u64,
    pub total_filtered_tokens: u64,
    pub total_saved_tokens: u64,
    pub avg_savings_pct: f64,
    pub top_commands: Vec<(String, u64, f64)>,
    pub daily: Vec<DayStats>,
}

#[derive(Debug)]
pub struct DayStats {
    pub date: String,
    pub commands: u64,
    pub saved: u64,
    pub raw: u64,
}

pub fn gain_summary(conn: &Connection, days: u32) -> Result<GainSummary> {
    ensure_table(conn)?;

    let mut summary = GainSummary::default();

    let row = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(raw_tokens), 0), COALESCE(SUM(filtered_tokens), 0)
         FROM proxy_tracking
         WHERE timestamp >= datetime('now', ?1)",
        [format!("-{days} days")],
        |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, u64>(2)?,
            ))
        },
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
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, u64>(1)?,
            r.get::<_, f64>(2)?,
        ))
    })?;
    summary.top_commands = rows.flatten().collect();

    // daily breakdown (last N days)
    let mut daily_stmt = conn.prepare(
        "SELECT DATE(timestamp) as day,
                COUNT(*) as cmds,
                SUM(raw_tokens - filtered_tokens) as saved,
                SUM(raw_tokens) as raw
         FROM proxy_tracking
         WHERE timestamp >= datetime('now', ?1)
         GROUP BY day
         ORDER BY day DESC
         LIMIT ?2",
    )?;
    let daily_rows = daily_stmt.query_map(
        rusqlite::params![format!("-{days} days"), days.min(14)],
        |r| {
            Ok(DayStats {
                date: r.get(0)?,
                commands: r.get(1)?,
                saved: r.get(2)?,
                raw: r.get(3)?,
            })
        },
    )?;
    summary.daily = daily_rows.flatten().collect();

    Ok(summary)
}

pub fn render_gain(summary: &GainSummary) -> String {
    if summary.total_commands == 0 {
        return "No proxy data yet. Run commands through `abyss proxy` to start tracking.\n".into();
    }

    let mut out = String::new();

    // Header
    out.push_str("╭─────────────────────────────────────────────╮\n");
    out.push_str(&format!(
        "│  abyss proxy — {:>7} tokens saved ({:.0}%)  │\n",
        fmt_num(summary.total_saved_tokens),
        summary.avg_savings_pct
    ));
    out.push_str("╰─────────────────────────────────────────────╯\n\n");

    out.push_str(&format!(
        "  {} commands proxied\n",
        fmt_num(summary.total_commands)
    ));
    out.push_str(&format!(
        "  {} raw  →  {} delivered  ({} saved)\n\n",
        fmt_num(summary.total_raw_tokens),
        fmt_num(summary.total_filtered_tokens),
        fmt_num(summary.total_saved_tokens),
    ));

    // Top commands
    if !summary.top_commands.is_empty() {
        out.push_str("  Top commands:\n");

        let max_saved = summary
            .top_commands
            .first()
            .map(|c| c.1)
            .unwrap_or(1)
            .max(1);

        for (cmd, saved, pct) in &summary.top_commands {
            let short = truncate_cmd(cmd, 28);
            let bar_len = ((*saved as f64 / max_saved as f64) * 16.0).round() as usize;
            let bar: String = "█".repeat(bar_len.min(16));
            let bar_pad: String = "░".repeat(16 - bar_len.min(16));
            out.push_str(&format!(
                "    {short:<28} {bar}{bar_pad} {:>6} ({pct:.0}%)\n",
                fmt_num(*saved),
            ));
        }
    }

    // Daily breakdown
    if summary.daily.len() > 1 {
        out.push_str("\n  Daily:\n");
        let max_daily = summary
            .daily
            .iter()
            .map(|d| d.saved)
            .max()
            .unwrap_or(1)
            .max(1);

        for day in &summary.daily {
            let pct = if day.raw > 0 {
                (day.saved as f64 / day.raw as f64) * 100.0
            } else {
                0.0
            };
            let bar_len = ((day.saved as f64 / max_daily as f64) * 12.0).round() as usize;
            let bar: String = "▓".repeat(bar_len.min(12));
            let bar_pad: String = "░".repeat(12 - bar_len.min(12));
            out.push_str(&format!(
                "    {} {:>3} cmds  {bar}{bar_pad} {:>6} saved ({pct:.0}%)\n",
                day.date,
                day.commands,
                fmt_num(day.saved),
            ));
        }
    }

    out
}

fn fmt_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}K", n as f64 / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn truncate_cmd(cmd: &str, max_len: usize) -> String {
    if cmd.len() <= max_len {
        return cmd.to_string();
    }
    // Try to keep the meaningful part: "cargo test -- ..." → "cargo test --..."
    let mut s = cmd[..max_len - 3].to_string();
    s.push_str("...");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_num_formats_correctly() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(1_500), "1.5K");
        assert_eq!(fmt_num(15_000), "15K");
        assert_eq!(fmt_num(345_501), "346K");
        assert_eq!(fmt_num(1_200_000), "1.2M");
    }

    #[test]
    fn truncate_cmd_short_passthrough() {
        assert_eq!(truncate_cmd("git status", 28), "git status");
    }

    #[test]
    fn truncate_cmd_long_truncates() {
        let long = "cat src/graph/languages/typescript.rs";
        let t = truncate_cmd(long, 28);
        assert!(t.len() <= 28);
        assert!(t.ends_with("..."));
    }

    #[test]
    fn render_gain_no_data() {
        let s = GainSummary::default();
        let out = render_gain(&s);
        assert!(out.contains("No proxy data"));
    }

    #[test]
    fn render_gain_with_data() {
        let s = GainSummary {
            total_commands: 70,
            total_raw_tokens: 503504,
            total_filtered_tokens: 158003,
            total_saved_tokens: 345501,
            avg_savings_pct: 68.6,
            top_commands: vec![
                ("cat src/main.rs".into(), 200254, 65.0),
                ("cargo test".into(), 8850, 100.0),
            ],
            daily: vec![],
        };
        let out = render_gain(&s);
        assert!(out.contains("346K"), "should format saved tokens: {out}");
        assert!(out.contains("69%"), "should show pct: {out}");
        assert!(
            out.contains("cat src/main.rs"),
            "should list top cmd: {out}"
        );
        assert!(out.contains("█"), "should have bar chart: {out}");
    }
}
