//! File-level pre-edit context: everything an agent should see before
//! touching a file. Shared by `abyss context`, `abyss hook pre-edit`,
//! and (eventually) the MCP server.

use anyhow::Result;

use crate::storage::Repository;

/// Collect the full pre-edit context for a file as JSON. Shared by
/// `abyss context` and `abyss hook pre-edit`. Returns None if the file
/// is not in the index.
pub fn build_file_context(repo: &Repository, file: &str) -> Result<Option<serde_json::Value>> {
    let conn = repo.conn();

    // Find file (exact relative path or suffix match)
    let file_id: i64 = match conn.query_row(
        "SELECT id FROM files WHERE path = ?1 OR path LIKE ?2",
        rusqlite::params![file, format!("%{file}")],
        |r| r.get(0),
    ) {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };

    let file_path: String = repo.get_file_path(file_id)?.unwrap_or_default();

    // Get all symbols defined in this file (used for the total count and to
    // ensure we expose symbols even when they have zero external callers).
    let symbols = repo.find_symbols_in_file(file_id)?;

    // Single JOIN replaces an N+1 cascade:
    //   N symbols × (1 find_callers_of + M is_test_file) round-trips.
    // We pull symbol × external-caller rows in one shot, with the per-source
    // file path and an inline is_test_file replica (path-LIKE patterns kept
    // 1:1 with Repository::is_test_file). Symbols with no external callers
    // simply don't appear in the join — same observable shape as the old loop.
    //
    // Confident matches (>= 0.7) are reported as callers; ambiguous ones are
    // listed separately so agents can tell solid ground from guesses.
    const CONTEXT_MIN_CONFIDENCE: f64 = 0.7;
    let mut callers_stmt = conn.prepare(
        "SELECT s.name, s.kind, s.line,
                r.source_line, r.source_symbol, r.confidence,
                sf.path,
                CASE
                    WHEN sf.path LIKE '%\\_test.%' ESCAPE '\\' THEN 1
                    WHEN sf.path LIKE '%/test/%' THEN 1
                    WHEN sf.path LIKE '%/tests/%' THEN 1
                    WHEN sf.path LIKE '%.test.%' THEN 1
                    WHEN sf.path LIKE '%.spec.%' THEN 1
                    ELSE 0
                END AS is_test
         FROM symbols s
         JOIN refs r
              ON r.target_name = s.name
             AND (r.target_file_id = s.file_id OR r.target_file_id IS NULL)
             AND r.kind IN ('call','field_access')
         JOIN files sf ON sf.id = r.source_file_id
         WHERE s.file_id = ?1
           AND s.kind IN ('function','method','struct','class','interface','type')
           AND r.source_file_id != ?1
         ORDER BY s.line, r.confidence DESC",
    )?;

    // Group rows by symbol (sym_name, sym_kind, sym_line) preserving the
    // ORDER BY emission order. SQLite returns symbols in line order, so we
    // can group with a simple "did the key change?" check — no HashMap.
    struct SymGroup {
        name: String,
        kind: String,
        line: u32,
        confident: Vec<serde_json::Value>,
        possible: Vec<serde_json::Value>,
    }
    let mut groups: Vec<SymGroup> = Vec::new();
    let mut rows = callers_stmt.query([file_id])?;
    while let Some(row) = rows.next()? {
        let sym_name: String = row.get(0)?;
        let sym_kind: String = row.get(1)?;
        let sym_line: u32 = row.get(2)?;
        let source_line: u32 = row.get(3)?;
        let source_symbol: Option<String> = row.get(4)?;
        let confidence: f64 = row.get(5)?;
        let source_file_path: String = row.get(6)?;
        let is_test: i64 = row.get(7)?;

        let entry = serde_json::json!({
            "file": source_file_path,
            "line": source_line + 1,
            "caller": source_symbol,
            "confidence": confidence,
            "is_test": is_test != 0,
        });

        let needs_new_group = match groups.last() {
            Some(g) => g.name != sym_name || g.kind != sym_kind || g.line != sym_line,
            None => true,
        };
        if needs_new_group {
            groups.push(SymGroup {
                name: sym_name,
                kind: sym_kind,
                line: sym_line,
                confident: Vec::new(),
                possible: Vec::new(),
            });
        }
        let g = groups.last_mut().expect("just pushed");
        if confidence >= CONTEXT_MIN_CONFIDENCE {
            g.confident.push(entry);
        } else {
            g.possible.push(entry);
        }
    }

    let sym_callers: Vec<serde_json::Value> = groups
        .into_iter()
        .map(|g| {
            serde_json::json!({
                "symbol": g.name,
                "kind": g.kind,
                "line": g.line + 1,
                "external_callers": g.confident,
                "possible_callers": g.possible,
            })
        })
        .collect();

    // Get outgoing refs (what this file depends on)
    let mut deps_stmt = conn.prepare(
        "SELECT DISTINCT r.target_name, f.path, r.kind
         FROM refs r LEFT JOIN files f ON r.target_file_id = f.id
         WHERE r.source_file_id = ?1 AND r.kind IN ('call','type_ref')
         AND r.target_file_id IS NOT NULL AND r.target_file_id != ?1
         LIMIT 20",
    )?;
    let deps: Vec<serde_json::Value> = deps_stmt
        .query_map([file_id], |row| {
            Ok(serde_json::json!({
                "name": row.get::<_, String>(0)?,
                "file": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Hotspot info
    let hotspot: Option<(f64, i64, f64)> = conn.query_row(
        "SELECT hotspot_score, change_count_30d, cyclomatic FROM file_metrics WHERE file_id = ?1",
        [file_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).ok();

    // Coupled files
    let mut coupling_stmt = conn.prepare(
        "SELECT file_b, co_changes, coupling_score FROM change_coupling WHERE file_a = ?1
         UNION SELECT file_a, co_changes, coupling_score FROM change_coupling WHERE file_b = ?1
         ORDER BY coupling_score DESC LIMIT 5",
    )?;
    let coupled: Vec<serde_json::Value> = coupling_stmt
        .query_map([&file_path], |row| {
            Ok(serde_json::json!({
                "file": row.get::<_, String>(0)?,
                "co_changes": row.get::<_, i64>(1)?,
                "coupling": format!("{:.0}%", row.get::<_, f64>(2)? * 100.0),
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(Some(serde_json::json!({
        "file": file_path,
        "symbols_defined": symbols.len(),
        "symbols_with_external_callers": sym_callers,
        "dependencies": deps,
        "hotspot": hotspot.map(|(score, changes, cc)| serde_json::json!({
            "score": score, "changes_30d": changes, "complexity": cc
        })),
        "coupled_files": coupled,
    })))
}

/// Pull a file path out of an agent tool-call JSON payload. Supports the
/// shapes used by Claude Code, Codex, Gemini, Pi, Hermes, and OpenClaw
/// hooks without needing a --platform flag.
pub fn extract_file_path(v: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 2] = ["file_path", "path"];
    for k in KEYS {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    for container in ["tool_input", "params", "arguments", "input", "tool_args"] {
        if let Some(c) = v.get(container) {
            for k in KEYS {
                if let Some(s) = c.get(k).and_then(|x| x.as_str()) {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::extract_file_path;
    use serde_json::json;

    #[test]
    fn extracts_from_known_payload_shapes() {
        for payload in [
            json!({"file_path": "a.rs"}),
            json!({"path": "a.rs"}),
            json!({"tool_input": {"file_path": "a.rs"}}),
            json!({"params": {"path": "a.rs"}}),
            json!({"arguments": {"file_path": "a.rs"}}),
            json!({"input": {"file_path": "a.rs"}}),
        ] {
            assert_eq!(
                extract_file_path(&payload).as_deref(),
                Some("a.rs"),
                "{payload}"
            );
        }
    }

    #[test]
    fn returns_none_for_pathless_payloads() {
        assert_eq!(
            extract_file_path(&serde_json::json!({"command": "ls"})),
            None
        );
        assert_eq!(extract_file_path(&serde_json::json!({})), None);
    }
}
