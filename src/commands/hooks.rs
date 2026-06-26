use anyhow::Result;

use crate::config::Config;
use crate::indexer::IndexPipeline;
use crate::storage::Repository;

pub const HOOK_LANGS: [&str; 17] = [
    "go", "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "java", "c", "h", "cpp", "cc",
    "cxx", "hpp",
];

/// Read JSON from stdin (used by hook entry points).
pub fn read_stdin_json() -> Option<serde_json::Value> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).ok()?;
    serde_json::from_str(buf.trim()).ok()
}

pub fn cmd_hook(config: Config, action: super::HookAction, json: bool) -> Result<()> {
    // Hooks must never block the agent: every early-out is a silent success.
    match action {
        super::HookAction::PreEdit => hook_pre_edit(config, json),
        super::HookAction::PostEdit => hook_post_edit(config),
        super::HookAction::ProxyRewrite => hook_proxy_rewrite(),
    }
}

/// Proxy rewrite hook: intercept Bash tool calls, rewrite commands to route
/// through `abyss proxy`. Reads JSON from stdin (Claude Code PreToolUse
/// payload), outputs hook response JSON to stdout.
///
/// Protocol (Claude Code PreToolUse):
/// - stdout JSON with `permissionDecision: "allow"` + `updatedInput` → rewritten
/// - empty stdout (exit 0) → passthrough unchanged
fn hook_proxy_rewrite() -> Result<()> {
    use crate::proxy::rewrite;

    let Some(payload) = read_stdin_json() else {
        return Ok(());
    };

    // Extract the command string from tool_input.command
    let cmd = payload
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    if cmd.is_empty() {
        return Ok(());
    }

    // Try to rewrite
    let Some(rewritten) = rewrite::rewrite_command(cmd) else {
        return Ok(()); // No rewrite available → passthrough
    };

    // Build the updatedInput with the rewritten command
    let tool_input = payload
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let mut updated_input = tool_input;
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("command".into(), serde_json::Value::String(rewritten));
    }

    let response = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "abyss proxy rewrite",
            "updatedInput": updated_input
        }
    });

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn hook_pre_edit(config: Config, json: bool) -> Result<()> {
    let start = std::time::Instant::now();
    let Some(payload) = read_stdin_json() else {
        return Ok(());
    };
    let Some(raw_path) = crate::context::extract_file_path(&payload) else {
        return Ok(());
    };
    let ext = raw_path.rsplit('.').next().unwrap_or("");
    if !HOOK_LANGS.contains(&ext) {
        return Ok(());
    }
    // Opt-in: only fire when the project has an index.
    if !config.db_path.exists() {
        return Ok(());
    }

    // Read-only path: hooks must never block the agent. A full structural
    // refresh on every PreToolUse blocks the agent on a repo scan before
    // every edit — the #1 reason ambient delivery was broken. Index updates
    // run from hook_post_edit instead; pre_edit only queries.
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;

    let rel = std::path::Path::new(&raw_path)
        .strip_prefix(&config.workspace)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| raw_path.replace('\\', "/"));

    let Some(mut ctx) = crate::context::build_file_context(&repo, &rel)? else {
        return Ok(());
    };

    // Best-effort enrichment for the card. All failures are silent — the
    // hook must never block the agent on a missing optional metric.
    enrich_ctx_for_card(&repo, &rel, &mut ctx);

    if json {
        println!("{}", serde_json::to_string(&ctx)?);
    }

    let staleness_ms = start.elapsed().as_millis();
    let card = crate::context::render_card(&ctx, &rel, staleness_ms);
    eprintln!("{card}");

    Ok(())
}

/// Add `siblings`, `epoch`, and `last_touched_days` to the context payload
/// so `render_card` has the data it needs. Every query is best-effort.
pub fn enrich_ctx_for_card(repo: &Repository, rel: &str, ctx: &mut serde_json::Value) {
    let conn = repo.conn();
    let obj = match ctx.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // ---- siblings: other files in the same directory ----
    let dir = match rel.rsplit_once('/') {
        Some((d, _)) => format!("{d}/"),
        None => String::new(),
    };
    let fname = rel.rsplit('/').next().unwrap_or(rel);
    let siblings: Vec<serde_json::Value> = (|| -> rusqlite::Result<Vec<serde_json::Value>> {
        let mut stmt = conn.prepare(
            "SELECT path FROM files WHERE dir = ?1 AND path != ?2 ORDER BY path LIMIT 12",
        )?;
        let rows = stmt.query_map([&dir, &rel.to_string()], |r| r.get::<_, String>(0))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .map(|p| serde_json::Value::String(p.rsplit('/').next().unwrap_or(&p).to_string()))
            .collect())
    })()
    .unwrap_or_default();
    if !siblings.is_empty() {
        obj.insert("siblings".into(), serde_json::Value::Array(siblings));
    }
    let _ = fname;

    // ---- epoch: latest known commit ts (workspace-wide, fast aggregate) ----
    if let Ok(epoch) =
        conn.query_row::<i64, _, _>("SELECT COALESCE(MAX(ts), 0) FROM commits", [], |r| r.get(0))
    {
        obj.insert("epoch".into(), serde_json::json!(epoch));
    }

    // ---- last_touched_days: based on file_metrics.last_changed_ts ----
    if let Ok(last_ts) = conn.query_row::<i64, _, _>(
        "SELECT COALESCE(fm.last_changed_ts, 0) FROM file_metrics fm
         JOIN files f ON f.id = fm.file_id WHERE f.path = ?1",
        [rel],
        |r| r.get(0),
    ) && last_ts > 0
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let days = ((now - last_ts) / 86_400).max(0);
        obj.insert("last_touched_days".into(), serde_json::json!(days));
    }
}

fn hook_post_edit(config: Config) -> Result<()> {
    if !config.db_path.exists() {
        return Ok(());
    }
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config);
    let _ = pipeline.run_structural(&repo);
    Ok(())
}
