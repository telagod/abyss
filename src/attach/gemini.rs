//! Install abyss hooks into Gemini CLI's `settings.json`.
//!
//! Gemini's event names and matchers are NOT the same as Claude's —
//! ground truth lives in `code-abyss/bin/lib/abyss-integration.js`
//! (`injectGeminiHooks`). The schema is:
//!
//! ```json
//! {
//!   "hooks": {
//!     "SessionStart": [{
//!       "matcher": "startup",
//!       "hooks": [{
//!         "name": "abyss-init",
//!         "type": "command",
//!         "command": "abyss hook pre-edit",
//!         "timeout": 10000,
//!         "description": "Auto-index project with abyss"
//!       }]
//!     }],
//!     "BeforeTool": [{
//!       "matcher": "write_file|replace|edit_file",
//!       "hooks": [{
//!         "name": "abyss-check",
//!         "type": "command",
//!         "command": "abyss hook pre-edit",
//!         "timeout": 5000,
//!         "description": "Check callers before editing code"
//!       }]
//!     }],
//!     "AfterTool": [{
//!       "matcher": "write_file|replace|edit_file",
//!       "hooks": [{
//!         "name": "abyss-post",
//!         "type": "command",
//!         "command": "abyss hook post-edit",
//!         "timeout": 5000,
//!         "description": "Reindex after edit"
//!       }]
//!     }]
//!   }
//! }
//! ```
//!
//! Timeout units are **milliseconds** for Gemini (Codex uses seconds —
//! don't confuse the two).
//!
//! Idempotency: hook entries keyed by `name` (`abyss-*`) are upserted
//! in place; entries authored by the user are preserved.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// (event, matcher, hook_name, command, timeout_ms, description)
const ENTRIES: &[(&str, &str, &str, &str, u32, &str)] = &[
    (
        "SessionStart",
        "startup",
        "abyss-init",
        "abyss hook pre-edit",
        10_000,
        "Auto-index project with abyss",
    ),
    (
        "BeforeTool",
        "write_file|replace|edit_file",
        "abyss-check",
        "abyss hook pre-edit",
        5_000,
        "Check callers before editing code",
    ),
    (
        "AfterTool",
        "write_file|replace|edit_file",
        "abyss-post",
        "abyss hook post-edit",
        5_000,
        "Reindex after edit",
    ),
];

/// Resolve the target `settings.json` path.
///
/// * `--local` → `<cwd>/.gemini/settings.json`
/// * default   → `<home>/.gemini/settings.json`
pub fn settings_path(local: bool) -> Result<PathBuf> {
    if local {
        let cwd = std::env::current_dir().context("cannot read current dir")?;
        return Ok(cwd.join(".gemini").join("settings.json"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
    })?;
    Ok(home.join(".gemini").join("settings.json"))
}

/// True iff every abyss-managed hook entry is present.
pub fn already_installed(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    ENTRIES
        .iter()
        .all(|(event, matcher, name, _, _, _)| has_named_hook(&value, event, matcher, name))
}

fn has_named_hook(root: &Value, event: &str, matcher: &str, name: &str) -> bool {
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter().any(|entry| {
                entry.get("matcher").and_then(Value::as_str) == Some(matcher)
                    && entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .map(|inner| {
                            inner
                                .iter()
                                .any(|h| h.get("name").and_then(Value::as_str) == Some(name))
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Install (or upgrade) the abyss hook entries.
pub fn install(local: bool) -> Result<()> {
    let path = settings_path(local)?;
    install_at(&path)
}

/// Test-friendly variant: install into an explicit `settings.json` path.
pub fn install_at(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut root: Value = if path.exists() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&raw)
                .with_context(|| format!("parsing {} as JSON", path.display()))?
        }
    } else {
        json!({})
    };

    if !root.is_object() {
        anyhow::bail!(
            "{} is not a JSON object — refusing to overwrite",
            path.display()
        );
    }

    {
        let hooks = root
            .as_object_mut()
            .expect("checked")
            .entry("hooks")
            .or_insert_with(|| json!({}));
        if !hooks.is_object() {
            anyhow::bail!("`hooks` field exists but is not an object");
        }

        for (event, matcher, name, command, timeout_ms, description) in ENTRIES {
            upsert_named_hook(
                hooks,
                event,
                matcher,
                name,
                command,
                *timeout_ms,
                description,
            )?;
        }
    }

    let pretty = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, pretty).with_context(|| format!("writing {}", path.display()))?;

    println!("✓ abyss hook installed at {}", path.display());
    for (event, matcher, name, _, timeout_ms, _) in ENTRIES {
        println!("  {event} ({matcher}) → {name} timeout={timeout_ms}ms");
    }
    println!("  shape: Gemini SessionStart + BeforeTool/AfterTool (ms timeouts)");
    Ok(())
}

/// Install the proxy-rewrite hook (BeforeTool, `run_shell_command` matcher)
/// on top of the existing hooks. Idempotent.
pub fn install_proxy(local: bool) -> Result<()> {
    let path = settings_path(local)?;
    install_proxy_at(&path)
}

pub fn install_proxy_at(path: &std::path::Path) -> Result<()> {
    // Ensure the base hooks exist first.
    install_at(path)?;

    const PROXY_NAME: &str = "abyss-proxy";
    const PROXY_CMD: &str = "abyss hook proxy-rewrite";
    const PROXY_MATCHER: &str = "run_shell_command";
    const PROXY_TIMEOUT: u32 = 5_000;
    const PROXY_DESC: &str = "Rewrite shell commands for token compression";

    // Re-read the file (install_at just wrote it)
    let raw = std::fs::read_to_string(path)?;
    let mut root: Value = serde_json::from_str(&raw)?;

    let hooks = root
        .as_object_mut()
        .expect("just validated")
        .entry("hooks")
        .or_insert_with(|| json!({}));

    // Check if already present
    if let Some(arr) = hooks.get("BeforeTool").and_then(Value::as_array) {
        let already = arr.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .map(|inner| {
                    inner
                        .iter()
                        .any(|h| h.get("name").and_then(Value::as_str) == Some(PROXY_NAME))
                })
                .unwrap_or(false)
        });
        if already {
            println!("  ProxyRewrite (already present): {PROXY_CMD}");
            return Ok(());
        }
    }

    upsert_named_hook(
        hooks,
        "BeforeTool",
        PROXY_MATCHER,
        PROXY_NAME,
        PROXY_CMD,
        PROXY_TIMEOUT,
        PROXY_DESC,
    )?;

    let pretty = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, &pretty)?;

    println!("  ProxyRewrite (added): {PROXY_CMD}");
    Ok(())
}

/// Upsert a single named hook. Existing matcher-block is reused; an
/// inner hook with the same `name` is replaced; otherwise we append.
fn upsert_named_hook(
    hooks: &mut Value,
    event: &str,
    matcher: &str,
    name: &str,
    command: &str,
    timeout_ms: u32,
    description: &str,
) -> Result<()> {
    let arr = hooks
        .as_object_mut()
        .expect("hooks object")
        .entry(event)
        .or_insert_with(|| json!([]));
    if !arr.is_array() {
        anyhow::bail!("`hooks.{event}` exists but is not an array");
    }
    let arr = arr.as_array_mut().expect("array");

    let new_entry = json!({
        "name": name,
        "type": "command",
        "command": command,
        "timeout": timeout_ms,
        "description": description,
    });

    for entry in arr.iter_mut() {
        if entry.get("matcher").and_then(Value::as_str) != Some(matcher) {
            continue;
        }
        let inner = entry
            .as_object_mut()
            .and_then(|o| o.entry("hooks").or_insert_with(|| json!([])).as_array_mut());
        let Some(inner) = inner else {
            anyhow::bail!("hook entry for matcher `{matcher}` has malformed inner hooks");
        };
        // Replace by name if present.
        for h in inner.iter_mut() {
            if h.get("name").and_then(Value::as_str) == Some(name) {
                *h = new_entry;
                return Ok(());
            }
        }
        inner.push(new_entry);
        return Ok(());
    }

    arr.push(json!({
        "matcher": matcher,
        "hooks": [new_entry]
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_named(root: &Value, event: &str, name: &str) -> Option<Value> {
        root["hooks"][event].as_array()?.iter().find_map(|entry| {
            entry["hooks"]
                .as_array()?
                .iter()
                .find(|h| h.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
    }

    #[test]
    fn install_at_writes_gemini_native_events() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gemini/settings.json");
        install_at(&path).unwrap();
        assert!(path.exists());

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        // SessionStart / BeforeTool / AfterTool (NOT PreToolUse / PostToolUse —
        // those are Claude-only event names).
        for ev in ["SessionStart", "BeforeTool", "AfterTool"] {
            assert!(
                v["hooks"].get(ev).is_some(),
                "missing Gemini event {ev}: {v}"
            );
        }
        assert!(v["hooks"].get("PreToolUse").is_none());
        assert!(v["hooks"].get("PostToolUse").is_none());

        // Matchers per ground truth.
        let session_entry = &v["hooks"]["SessionStart"][0];
        assert_eq!(session_entry["matcher"], "startup");
        let before_entry = &v["hooks"]["BeforeTool"][0];
        assert_eq!(before_entry["matcher"], "write_file|replace|edit_file");

        // Hook entries are name/type/command/timeout(ms)/description objects.
        let init = find_named(&v, "SessionStart", "abyss-init").expect("abyss-init present");
        assert_eq!(init["type"], "command");
        assert_eq!(init["command"], "abyss hook pre-edit");
        assert_eq!(init["timeout"], 10_000);
        assert!(init.get("description").is_some());

        let check = find_named(&v, "BeforeTool", "abyss-check").expect("abyss-check present");
        assert_eq!(check["timeout"], 5_000);

        let post = find_named(&v, "AfterTool", "abyss-post").expect("abyss-post present");
        assert_eq!(post["command"], "abyss hook post-edit");

        assert!(already_installed(&path));
    }

    #[test]
    fn install_at_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        for _ in 0..3 {
            install_at(&path).unwrap();
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Exactly one matcher block per event, one named inner hook each.
        for ev in ["SessionStart", "BeforeTool", "AfterTool"] {
            assert_eq!(v["hooks"][ev].as_array().unwrap().len(), 1);
            assert_eq!(v["hooks"][ev][0]["hooks"].as_array().unwrap().len(), 1);
        }
    }

    #[test]
    fn install_preserves_unrelated_top_level_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","model":"gemini-2.5-pro","other":{"keep":"me"}}"#,
        )
        .unwrap();
        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["model"], "gemini-2.5-pro");
        assert_eq!(v["other"]["keep"], "me");
        assert!(
            v["hooks"]["SessionStart"][0]["hooks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["name"] == "abyss-init")
        );
    }

    #[test]
    fn install_at_accepts_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "").unwrap();
        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(find_named(&v, "BeforeTool", "abyss-check").is_some());
    }

    #[test]
    fn user_authored_hooks_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"name":"my-init","type":"command","command":"echo hi","timeout":1000}]}]}}"#,
        )
        .unwrap();
        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // User's `my-init` survives, our `abyss-init` is appended to the same matcher block.
        let inner = v["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap();
        assert!(inner.iter().any(|h| h["name"] == "my-init"));
        assert!(inner.iter().any(|h| h["name"] == "abyss-init"));
    }

    #[test]
    fn already_installed_detects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert!(!already_installed(&path));
        install_at(&path).unwrap();
        assert!(already_installed(&path));
    }
}
