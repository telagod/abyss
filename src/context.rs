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

    // Find file via the shared fuzzy resolver: exact match wins, then
    // root-anchored prefix matches, then suffix matches by shortest path.
    // This keeps `abyss context src/hono.ts` from binding to a vendored copy
    // like `benchmarks/jsx/src/hono.ts`.
    let (file_id, file_path) = match repo.find_file_fuzzy(file)? {
        Some(row) => row,
        None => return Ok(None),
    };

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

    // ─── Arch facts (L0 coordinates) ───
    // Populated by IndexPipeline::run_structural at the end of every pass.
    // Older indexes (pre-v5 schema) won't have these rows yet — `get_arch_fact`
    // returns None in that case and we omit the field so the card's "where"
    // line falls back to the placeholder.
    let arch_block = match repo.get_arch_fact(file_id)? {
        Some(fact) => {
            let module = repo.get_arch_module(fact.module_id)?;
            let module_label = module.as_ref().map(|m| m.label.clone()).unwrap_or_else(|| {
                if fact.module_id < 0 {
                    String::from("—")
                } else {
                    format!("module-{}", fact.module_id)
                }
            });
            Some(serde_json::json!({
                "layer": fact.layer,
                "role": fact.role,
                "module_id": fact.module_id,
                "module_label": module_label,
                "depth_from_entry": fact.depth_from_entry,
                "centrality": fact.centrality,
                "in_degree": fact.in_degree,
                "out_degree": fact.out_degree,
                "layer_conf": fact.layer_conf,
                "signals": fact.signals,
            }))
        }
        None => None,
    };

    Ok(Some(serde_json::json!({
        "file": file_path,
        "symbols_defined": symbols.len(),
        "symbols_with_external_callers": sym_callers,
        "dependencies": deps,
        "hotspot": hotspot.map(|(score, changes, cc)| serde_json::json!({
            "score": score, "changes_30d": changes, "complexity": cc
        })),
        "coupled_files": coupled,
        "arch": arch_block,
    })))
}

/// One-line `where` summary for the pre-edit card / CLI. Reads the arch_facts
/// row for `file` (relative path, suffix-matchable) and returns a structured
/// JSON view:
///
/// ```json
/// {
///   "file": "src/auth/login.go",
///   "layer": "domain",
///   "role": "core",
///   "module_label": "auth",
///   "module_id": 3,
///   "depth_from_entry": 2,
///   "centrality": 0.71,
///   "in_degree": 12, "out_degree": 7,
///   "layer_conf": 0.84,
///   "signals": { "dir": [...], "name": [...], "entry": false }
/// }
/// ```
///
/// Returns `Ok(None)` when the file is not indexed (so the caller can print
/// a friendly "not found" message instead of a cryptic SQL error).
pub fn where_summary(repo: &Repository, file: &str) -> Result<Option<serde_json::Value>> {
    let Some((file_id, file_path)) = repo.find_file_fuzzy(file)? else {
        return Ok(None);
    };

    let Some(fact) = repo.get_arch_fact(file_id)? else {
        return Ok(Some(serde_json::json!({
            "file": file_path,
            "layer": "unknown",
            "role": "unknown",
            "module_label": "—",
            "module_id": -1,
            "depth_from_entry": null,
            "centrality": 0.0,
            "in_degree": 0,
            "out_degree": 0,
            "layer_conf": 0.0,
            "signals": null,
            "note": "no arch_facts row for this file — reindex with current binary",
        })));
    };
    let module = repo.get_arch_module(fact.module_id)?;
    let module_label = module.as_ref().map(|m| m.label.clone()).unwrap_or_else(|| {
        if fact.module_id < 0 {
            String::from("—")
        } else {
            format!("module-{}", fact.module_id)
        }
    });

    Ok(Some(serde_json::json!({
        "file": file_path,
        "layer": fact.layer,
        "role": fact.role,
        "module_id": fact.module_id,
        "module_label": module_label,
        "depth_from_entry": fact.depth_from_entry,
        "centrality": fact.centrality,
        "in_degree": fact.in_degree,
        "out_degree": fact.out_degree,
        "layer_conf": fact.layer_conf,
        "signals": fact.signals,
    })))
}

/// Render the pre-edit context as a structured `<abyss-card>` system-reminder
/// block. Output is meant to be written to stderr by the hook — Claude Code
/// surfaces hook stderr to the agent verbatim, so this is how we get a card
/// into the agent's view without any tool-result plumbing.
///
/// The card is best-effort: every section is skipped cleanly if its data is
/// missing from `ctx`. Body is hard-capped at `BODY_BUDGET_CHARS` (~1800
/// tokens at 4 chars/token) so it never crowds the conversation.
pub fn render_card(ctx: &serde_json::Value, file_path: &str, staleness_ms: u128) -> String {
    const BODY_BUDGET_CHARS: usize = 7200;
    const HIGH_BLAST_RADIUS: usize = 10;

    let epoch = ctx
        .get("epoch")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);

    // L0 architectural coordinates live under ctx.arch (populated by
    // enrich_ctx_for_card via where_summary). Fall back to "unknown" / dir
    // path only if the arch step hasn't run on this index yet.
    let arch = ctx.get("arch");
    let layer = arch
        .and_then(|a| a.get("layer"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let module = arch
        .and_then(|a| a.get("module_label"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            file_path
                .rsplit_once('/')
                .map(|(dir, _)| dir)
                .unwrap_or("unknown")
        });
    let role = arch
        .and_then(|a| a.get("role"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let centrality = arch
        .and_then(|a| a.get("centrality"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let layer_conf = arch
        .and_then(|a| a.get("layer_conf"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let depth = arch
        .and_then(|a| a.get("depth_from_entry"))
        .and_then(serde_json::Value::as_i64);
    let in_deg = arch
        .and_then(|a| a.get("in_degree"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let out_deg = arch
        .and_then(|a| a.get("out_degree"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);

    let empty = Vec::new();
    let sym_callers = ctx["symbols_with_external_callers"]
        .as_array()
        .unwrap_or(&empty);
    let deps = ctx["dependencies"].as_array().unwrap_or(&empty);
    let coupled = ctx["coupled_files"].as_array().unwrap_or(&empty);
    let siblings = ctx["siblings"].as_array().unwrap_or(&empty);

    let mut body = String::new();

    // ---- header line: where am I in the system ----
    let sib_count = siblings.len();
    let sib_preview = siblings
        .iter()
        .take(6)
        .filter_map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    body.push_str(&format!(
        "where\n  layer={layer} · module={module} · role={role} · conf={layer_conf:.2}\n"
    ));
    let depth_str = depth.map(|d| d.to_string()).unwrap_or_else(|| "—".into());
    body.push_str(&format!(
        "  depth_from_entry={depth_str} · centrality={centrality:.2} · in={in_deg} out={out_deg}\n"
    ));
    if sib_count > 0 {
        body.push_str(&format!("  siblings({sib_count}): {sib_preview}"));
        if sib_count > 6 {
            body.push_str(&format!(" … +{} more", sib_count - 6));
        }
        body.push('\n');
    }

    // ---- depends-on: outgoing deps, grouped per target file ----
    if !deps.is_empty() {
        body.push_str("\ndepends-on (top 6)\n");
        // Group by file → list of names.
        let mut by_file: Vec<(String, Vec<String>)> = Vec::new();
        for d in deps {
            let file = d["file"].as_str().unwrap_or("?").to_string();
            let name = d["name"].as_str().unwrap_or("?").to_string();
            match by_file.iter_mut().find(|(f, _)| f == &file) {
                Some((_, ns)) => ns.push(name),
                None => by_file.push((file, vec![name])),
            }
        }
        let total = by_file.len();
        for (file, mut names) in by_file.iter().take(6).cloned().collect::<Vec<_>>() {
            names.sort();
            names.dedup();
            let n = names.len();
            let shown = names.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let suffix = if n > 4 {
                format!(", +{} more", n - 4)
            } else {
                String::new()
            };
            body.push_str(&format!("  → {file} :{shown}{suffix}  ({n} refs)\n"));
        }
        if total > 6 {
            body.push_str(&format!("  +{} more\n", total - 6));
        }
    }

    // ---- depended-on: incoming production callers, with blast-radius flag ----
    let mut prod_callers = 0usize;
    let mut test_callers = 0usize;
    let mut callers_per_file: Vec<(String, usize)> = Vec::new();
    for s in sym_callers {
        if let Some(arr) = s["external_callers"].as_array() {
            for c in arr {
                let is_test = c["is_test"].as_bool().unwrap_or(false);
                let file = c["file"].as_str().unwrap_or("?").to_string();
                if is_test {
                    test_callers += 1;
                } else {
                    prod_callers += 1;
                    match callers_per_file.iter_mut().find(|(f, _)| f == &file) {
                        Some((_, n)) => *n += 1,
                        None => callers_per_file.push((file, 1)),
                    }
                }
            }
        }
    }
    if prod_callers > 0 || test_callers > 0 {
        let blast = if prod_callers >= HIGH_BLAST_RADIUS {
            "  ⚠ HIGH BLAST RADIUS"
        } else {
            ""
        };
        body.push_str(&format!("\ndepended-on{blast}\n"));
        body.push_str(&format!(
            "  ← {prod_callers} prod callers across {} files, {test_callers} test callers\n",
            callers_per_file.len()
        ));
        callers_per_file.sort_by_key(|x| std::cmp::Reverse(x.1));
        let hottest = callers_per_file
            .iter()
            .take(3)
            .map(|(f, n)| format!("{f}({n})"))
            .collect::<Vec<_>>()
            .join(", ");
        if !hottest.is_empty() {
            body.push_str(&format!("  hottest: {hottest}\n"));
        }
    }

    // ---- contracts: exported symbols + their reach ----
    if !sym_callers.is_empty() {
        body.push_str("\ncontracts (exported symbols & callers)\n");
        for s in sym_callers {
            let name = s["symbol"].as_str().unwrap_or("?");
            let kind = s["kind"].as_str().unwrap_or("");
            let (n_prod, n_test) = s["external_callers"]
                .as_array()
                .map(|arr| {
                    arr.iter().fold((0usize, 0usize), |(p, t), c| {
                        if c["is_test"].as_bool().unwrap_or(false) {
                            (p, t + 1)
                        } else {
                            (p + 1, t)
                        }
                    })
                })
                .unwrap_or((0, 0));
            body.push_str(&format!(
                "  {name} ({kind}) → [{n_prod} callers, {n_test} tests]\n"
            ));
        }
    }

    // ---- change-coupling: files that historically move together ----
    if !coupled.is_empty() {
        body.push_str("\nchange-coupling (90d)\n");
        for c in coupled.iter().take(6) {
            let f = c["file"].as_str().unwrap_or("?");
            let coupling = c["coupling"].as_str().unwrap_or("?");
            let co = c["co_changes"].as_i64().unwrap_or(0);
            body.push_str(&format!("  {f}  {coupling} ({co} co-changes)\n"));
        }
    }

    // ---- recent activity / hotspot ----
    if let Some(h) = ctx.get("hotspot").filter(|v| !v.is_null()) {
        let score = h["score"].as_f64().unwrap_or(0.0);
        let changes = h["changes_30d"].as_i64().unwrap_or(0);
        body.push_str("\nrecent activity\n");
        body.push_str(&format!("  {changes} commits/30d"));
        if let Some(last) = ctx.get("last_touched_days").and_then(|v| v.as_i64()) {
            body.push_str(&format!(" · last touched {last}d ago"));
        }
        body.push('\n');
        if score > 0.0 {
            body.push_str(&format!("  hotspot {score:.0}\n"));
        }
    }

    // ---- resolver line: confidence floor of this card ----
    let ambiguous: usize = sym_callers
        .iter()
        .map(|s| s["possible_callers"].as_array().map(Vec::len).unwrap_or(0))
        .sum();
    body.push_str(&format!(
        "\nresolver: degraded=false · {ambiguous} ambiguous refs in this file\n"
    ));

    // ---- hard truncate so the card never crowds the agent's window ----
    let truncated = if body.len() > BODY_BUDGET_CHARS {
        let mut s = body[..BODY_BUDGET_CHARS].to_string();
        // Round to the last newline so we don't cut a line in half.
        if let Some(nl) = s.rfind('\n') {
            s.truncate(nl + 1);
        }
        s.push_str("… (truncated to fit budget)\n");
        s
    } else {
        body
    };

    format!(
        "<abyss-card file=\"{file_path}\" epoch=\"{epoch}\" staleness_ms=\"{staleness_ms}\" precision_mode=\"heuristic\">\n{truncated}</abyss-card>"
    )
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

    use super::render_card;

    #[test]
    fn render_card_emits_opening_and_closing_tags() {
        let ctx = json!({
            "file": "src/auth/login.go",
            "symbols_defined": 0,
            "symbols_with_external_callers": [],
            "dependencies": [],
            "hotspot": null,
            "coupled_files": [],
        });
        let card = render_card(&ctx, "src/auth/login.go", 42);
        assert!(card.starts_with("<abyss-card file=\"src/auth/login.go\""));
        assert!(card.contains("staleness_ms=\"42\""));
        assert!(card.contains("precision_mode=\"heuristic\""));
        assert!(card.ends_with("</abyss-card>"));
    }

    #[test]
    fn render_card_flags_high_blast_radius() {
        let callers: Vec<_> = (0..12)
            .map(|i| {
                json!({
                    "file": format!("src/caller_{i}.go"),
                    "line": 1,
                    "caller": "Foo",
                    "confidence": 1.0,
                    "is_test": false,
                })
            })
            .collect();
        let ctx = json!({
            "file": "src/auth/login.go",
            "symbols_defined": 1,
            "symbols_with_external_callers": [{
                "symbol": "Login",
                "kind": "function",
                "line": 10,
                "external_callers": callers,
                "possible_callers": [],
            }],
            "dependencies": [],
            "hotspot": null,
            "coupled_files": [],
        });
        let card = render_card(&ctx, "src/auth/login.go", 10);
        assert!(card.contains("HIGH BLAST RADIUS"));
        assert!(card.contains("12 prod callers across 12 files"));
    }

    #[test]
    fn render_card_truncates_when_oversized() {
        // Build a pathological ctx with many deps to exceed the budget.
        let deps: Vec<_> = (0..2000)
            .map(|i| {
                json!({
                    "name": format!("sym_{i}"),
                    "file": format!("src/dep_{i:04}.go"),
                    "kind": "call",
                })
            })
            .collect();
        let ctx = json!({
            "file": "src/big.go",
            "symbols_defined": 0,
            "symbols_with_external_callers": [],
            "dependencies": deps,
            "hotspot": null,
            "coupled_files": [],
        });
        let card = render_card(&ctx, "src/big.go", 0);
        // Total card length should stay within budget + small header/footer.
        assert!(card.len() < 8200, "card too long: {} chars", card.len());
        assert!(card.ends_with("</abyss-card>"));
    }
}
