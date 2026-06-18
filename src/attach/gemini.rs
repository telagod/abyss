//! Install abyss hooks into Gemini CLI's `settings.json`.
//!
//! Layout (only touches the `hooks` subtree, leaves the rest alone):
//!
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse":  [{ "matcher": "Edit|Write",
//!                       "hooks": [{ "type": "command", "command": "abyss hook pre-edit"  }] }],
//!     "PostToolUse": [{ "matcher": "Edit|Write",
//!                       "hooks": [{ "type": "command", "command": "abyss hook post-edit" }] }]
//!   }
//! }
//! ```
//!
//! Shape mirrors Claude Code's `settings.json` because Gemini CLI's
//! hook surface is the closest analogue. Best-effort schema — if your
//! Gemini CLI version uses different keys, please file an issue at
//! <https://github.com/telagod/abyss/issues>.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

const MATCHER: &str = "Edit|Write";
const PRE_CMD: &str = "abyss hook pre-edit";
const POST_CMD: &str = "abyss hook post-edit";

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

/// True iff the file already contains both abyss hook commands.
pub fn already_installed(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    has_command(&value, "PreToolUse", PRE_CMD) && has_command(&value, "PostToolUse", POST_CMD)
}

fn has_command(root: &Value, event: &str, cmd: &str) -> bool {
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter().any(|entry| {
                entry
                    .get("hooks")
                    .and_then(Value::as_array)
                    .map(|inner| {
                        inner
                            .iter()
                            .any(|h| h.get("command").and_then(Value::as_str) == Some(cmd))
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

    let hooks = root
        .as_object_mut()
        .expect("checked")
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        anyhow::bail!("`hooks` field exists but is not an object");
    }

    let pre_added = upsert_hook(hooks, "PreToolUse", MATCHER, PRE_CMD)?;
    let post_added = upsert_hook(hooks, "PostToolUse", MATCHER, POST_CMD)?;

    let pretty = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, pretty).with_context(|| format!("writing {}", path.display()))?;

    println!("✓ abyss hook installed at {}", path.display());
    println!(
        "  PreToolUse  ({}): {PRE_CMD}",
        if pre_added {
            "added"
        } else {
            "already present"
        }
    );
    println!(
        "  PostToolUse ({}): {POST_CMD}",
        if post_added {
            "added"
        } else {
            "already present"
        }
    );
    println!("  note: Gemini CLI hook schema is evolving — if this layout doesn't");
    println!("        match your version, please file an issue.");
    Ok(())
}

fn upsert_hook(hooks: &mut Value, event: &str, matcher: &str, command: &str) -> Result<bool> {
    let arr = hooks
        .as_object_mut()
        .expect("hooks object")
        .entry(event)
        .or_insert_with(|| json!([]));
    if !arr.is_array() {
        anyhow::bail!("`hooks.{event}` exists but is not an array");
    }
    let arr = arr.as_array_mut().expect("array");

    for entry in arr.iter_mut() {
        let entry_matcher = entry.get("matcher").and_then(Value::as_str).unwrap_or("");
        if entry_matcher != matcher {
            continue;
        }
        let inner = entry
            .as_object_mut()
            .and_then(|o| o.entry("hooks").or_insert_with(|| json!([])).as_array_mut());
        let Some(inner) = inner else {
            anyhow::bail!("hook entry for matcher `{matcher}` has malformed inner hooks");
        };
        let already = inner
            .iter()
            .any(|h| h.get("command").and_then(Value::as_str) == Some(command));
        if already {
            return Ok(false);
        }
        inner.push(json!({ "type": "command", "command": command }));
        return Ok(true);
    }

    arr.push(json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": command }]
    }));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_cmds(root: &Value, event: &str, cmd: &str) -> usize {
        root["hooks"][event]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .flat_map(|entry| {
                        entry["hooks"]
                            .as_array()
                            .into_iter()
                            .flat_map(|inner| inner.iter())
                    })
                    .filter(|h| h.get("command").and_then(Value::as_str) == Some(cmd))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn install_at_writes_settings_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".gemini/settings.json");
        install_at(&path).unwrap();
        assert!(path.exists());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);

        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);
    }

    #[test]
    fn install_preserves_unrelated_top_level_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"dark","model":"gemini-2.5-pro"}"#).unwrap();

        install_at(&path).unwrap();

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["model"], "gemini-2.5-pro");
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
    }

    #[test]
    fn install_at_accepts_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "").unwrap();
        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
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
