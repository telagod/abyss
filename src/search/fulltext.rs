use anyhow::Result;
use rusqlite::params;

use crate::storage::Repository;

/// Tuning knob: centrality boost multiplier. A file at centrality=0.5 gets
/// (1 + 2.0 * 0.5) = 2.0x weight, centrality=0.1 gets ~1.2x. Tuned against
/// the hono dogfood session — reference impls (high centrality) should out-rank
/// the test/import-mention siblings without burying long-tail correct hits.
const CENTRALITY_ALPHA: f64 = 2.0;

/// Test-path penalty. Tests aren't worthless, just secondary when the user
/// didn't specifically search for them. 0.5 keeps them in the list, just lower.
const TEST_PENALTY: f64 = 0.5;

/// `import` chunks almost never contain the user's target — they're a name
/// mention, not an implementation. Heavily demote.
const IMPORT_PENALTY: f64 = 0.3;

#[derive(Debug, Clone, serde::Serialize)]
pub struct FulltextResult {
    pub chunk_id: i64,
    pub score: f64,
    pub snippet: String,
}

pub fn search(repo: &Repository, query: &str, limit: usize) -> Result<Vec<FulltextResult>> {
    let conn = repo.conn();
    let fts_query = build_fts_query(query);

    // bm25 returns a NEGATIVE relevance number — smaller (more negative) is
    // more relevant. To boost a chunk we want its score to become MORE
    // negative, so we multiply by (1 + alpha * centrality) > 1. To penalize
    // (test files, import chunks) we multiply by a value < 1, which makes the
    // negative score closer to zero — i.e. less relevant. Both directions are
    // a single ASC sort.
    let mut stmt = conn.prepare(
        "SELECT c.id,
                bm25(chunks_fts) * \
                    (CASE \
                        WHEN f.path LIKE '%_test.%' OR f.path LIKE '%.test.%' \
                          OR f.path LIKE '%/test%' OR f.path LIKE '%/__tests__/%' \
                        THEN ?3 ELSE 1.0 \
                     END) * \
                    (CASE WHEN c.kind = 'import' THEN ?4 ELSE 1.0 END) * \
                    (1.0 + ?5 * COALESCE(af.centrality, 0)) AS ranked,
                snippet(chunks_fts, 0, '>>>', '<<<', '...', 64)
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         JOIN files  f ON f.id = c.file_id
         LEFT JOIN arch_facts af ON af.file_id = c.file_id
         WHERE chunks_fts MATCH ?1
         ORDER BY ranked ASC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(
        params![
            fts_query,
            limit as i64,
            TEST_PENALTY,
            IMPORT_PENALTY,
            CENTRALITY_ALPHA,
        ],
        |row| {
            let ranked: f64 = row.get(1)?;
            Ok(FulltextResult {
                chunk_id: row.get(0)?,
                // Flip sign so callers see "bigger is better" (matches the
                // fusion stage convention).
                score: -ranked,
                snippet: row.get(2)?,
            })
        },
    )?;

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
