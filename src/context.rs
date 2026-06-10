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

    // Get all symbols defined in this file
    let symbols = repo.find_symbols_in_file(file_id)?;

    // For each symbol, find external callers. Confident matches (>= 0.7) are
    // reported as callers; ambiguous ones are listed separately so agents can
    // tell solid ground from guesses.
    const CONTEXT_MIN_CONFIDENCE: f64 = 0.7;
    let caller_json = |c: &crate::storage::repo::RefRecord| {
        serde_json::json!({
            "file": &c.source_file_path,
            "line": c.source_line + 1,
            "caller": &c.source_symbol,
            "confidence": c.confidence,
            "is_test": repo.is_test_file(c.source_file_id).unwrap_or(false),
        })
    };
    let mut sym_callers: Vec<serde_json::Value> = Vec::new();
    for sym in &symbols {
        let callers = repo.find_callers_of(&sym.name, Some(file_id), 20)?;
        let external: Vec<_> = callers
            .iter()
            .filter(|c| c.source_file_id != file_id)
            .collect();
        if !external.is_empty() {
            let (confident, possible): (Vec<_>, Vec<_>) = external
                .into_iter()
                .partition(|c| c.confidence >= CONTEXT_MIN_CONFIDENCE);
            sym_callers.push(serde_json::json!({
                "symbol": sym.name,
                "kind": sym.kind,
                "line": sym.line + 1,
                "external_callers": confident.into_iter().map(caller_json).collect::<Vec<_>>(),
                "possible_callers": possible.into_iter().map(caller_json).collect::<Vec<_>>(),
            }));
        }
    }

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
