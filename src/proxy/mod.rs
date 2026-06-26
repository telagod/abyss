//! `abyss proxy <command>` — intercept, compress, and semantically enrich
//! command output before it reaches the agent.
//!
//! Two-tier filter design (inspired by RTK):
//! - **Rust handlers** for high-value commands (git, cargo, npm, pytest…)
//!   with structural parsing and optional semantic enrichment from the index.
//! - **TOML declarative filters** for the long tail — regex-driven 8-stage
//!   pipeline that covers 60+ commands without custom Rust code.
//!
//! The `never_worse` guard guarantees compressed output is always ≤ raw
//! token count. If filtering inflates, the raw output passes through.

pub mod filter;
pub mod handlers;
pub mod rewrite;
pub mod runner;
pub mod tee;
pub mod tracking;

use crate::config::Config;
use crate::storage::Repository;

/// Semantic context from the abyss index, injected into handlers.
#[derive(Debug, Default)]
pub struct ProxyContext {
    pub hotspots: Vec<(String, f64)>,
    pub impacted_callers: Vec<(String, u32)>,
    pub coupled_files: Vec<(String, f64)>,
}

impl ProxyContext {
    pub fn from_index(config: &Config, files: &[&str]) -> Option<Self> {
        if !config.db_path.exists() {
            return None;
        }
        let repo = Repository::open(&config.db_path, config.model.dimensions).ok()?;
        let conn = repo.conn();
        let mut ctx = ProxyContext::default();

        for &file in files {
            // hotspot score
            if let Ok(score) = conn.query_row(
                "SELECT COALESCE(fm.hotspot_score, 0.0)
                 FROM file_metrics fm JOIN files f ON f.id = fm.file_id
                 WHERE f.path = ?1",
                [file],
                |r| r.get::<_, f64>(0),
            ) && score > 0.0
            {
                ctx.hotspots.push((file.to_string(), score));
            }

            // caller count (blast radius)
            if let Ok(count) = conn.query_row(
                "SELECT COUNT(DISTINCT r.source_file_id)
                 FROM refs r JOIN symbols s ON s.id = r.target_symbol_id
                 JOIN files f ON f.id = s.file_id
                 WHERE f.path = ?1 AND r.confidence >= 0.7",
                [file],
                |r| r.get::<_, u32>(0),
            ) && count > 0
            {
                ctx.impacted_callers.push((file.to_string(), count));
            }

            // change coupling
            if let Ok(mut stmt) = conn.prepare(
                "SELECT cc.file_b, cc.coupling_score
                 FROM change_coupling cc
                 WHERE cc.file_a = ?1 AND cc.coupling_score > 0.3
                 ORDER BY cc.coupling_score DESC LIMIT 3",
            ) && let Ok(rows) = stmt.query_map([file], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
            }) {
                for row in rows.flatten() {
                    ctx.coupled_files.push(row);
                }
            }
        }

        if ctx.hotspots.is_empty()
            && ctx.impacted_callers.is_empty()
            && ctx.coupled_files.is_empty()
        {
            return None;
        }
        Some(ctx)
    }

    pub fn render_annotations(&self) -> String {
        let mut out = String::new();
        for (file, count) in &self.impacted_callers {
            if *count >= 5 {
                out.push_str(&format!("⚠ {file}: {count} callers (high blast radius)\n"));
            }
        }
        for (file, score) in &self.hotspots {
            if *score > 5.0 {
                out.push_str(&format!("🔥 {file}: hotspot score {score:.1}\n"));
            }
        }
        for (file, score) in &self.coupled_files {
            out.push_str(&format!("🔗 coupled: {file} ({score:.0}%)\n"));
        }
        out
    }
}

/// Token estimation: 4 chars per token (same heuristic as RTK).
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Never-worse guard: if filtered output is longer than raw, return raw.
pub fn never_worse<'a>(raw: &'a str, filtered: &'a str) -> &'a str {
    if estimate_tokens(filtered) >= estimate_tokens(raw) {
        raw
    } else {
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn never_worse_returns_shorter() {
        let raw = "a".repeat(100);
        let filtered = "b".repeat(50);
        assert_eq!(never_worse(&raw, &filtered), filtered);
    }

    #[test]
    fn never_worse_falls_back_when_inflated() {
        let raw = "short";
        let filtered = "this is actually longer than the raw output";
        assert_eq!(never_worse(raw, filtered), raw);
    }
}
