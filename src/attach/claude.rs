//! Install abyss hooks into Claude Code's `settings.json`.
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
//! Idempotent: a hook entry with the same `command` string is never duplicated.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

const MATCHER: &str = "Edit|Write";
const PRE_CMD: &str = "abyss hook pre-edit";
const POST_CMD: &str = "abyss hook post-edit";

/// Resolve the target `settings.json` path.
///
/// * `--local` → `<cwd>/.claude/settings.json`
/// * default   → `<home>/.claude/settings.json`, where `<home>` is the platform
///   home directory: `$HOME` on Unix, `%USERPROFILE%` on Windows. Resolved
///   through the `dirs` crate so Claude Code on Windows finds the same
///   `~/.claude/settings.json` it created.
fn settings_path(local: bool) -> Result<PathBuf> {
    if local {
        let cwd = std::env::current_dir().context("cannot read current dir")?;
        return Ok(cwd.join(".claude").join("settings.json"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
    })?;
    Ok(home.join(".claude").join("settings.json"))
}

/// Install (or upgrade) the abyss hook entries. Returns the path that was
/// written so the caller can echo it back to the user.
pub fn install(local: bool) -> Result<()> {
    let path = settings_path(local)?;
    install_at(&path)
}

/// Test-friendly variant: install into an explicit settings.json path.
/// Kept `pub(crate)` so external callers go through [`install`] which
/// applies the host policy (`--local` vs `$HOME`).
pub(crate) fn install_at(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut root: Value = if path.exists() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        // Accept an empty file as `{}` — first-time users often `touch` it.
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

    let (pre_added, _) = upsert_hook(hooks, "PreToolUse", MATCHER, PRE_CMD)?;
    let (post_added, _) = upsert_hook(hooks, "PostToolUse", MATCHER, POST_CMD)?;

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
    Ok(())
}

/// Ensure a `{matcher, hooks:[{type:command, command:CMD}]}` entry exists
/// under `hooks[event]`. Returns `(added, matched_existing_matcher)`.
fn upsert_hook(
    hooks: &mut Value,
    event: &str,
    matcher: &str,
    command: &str,
) -> Result<(bool, bool)> {
    let arr = hooks
        .as_object_mut()
        .expect("hooks object")
        .entry(event)
        .or_insert_with(|| json!([]));
    if !arr.is_array() {
        anyhow::bail!("`hooks.{event}` exists but is not an array");
    }
    let arr = arr.as_array_mut().expect("array");

    // 1. Look for an existing entry with the same matcher; if its inner
    //    `hooks` list already contains our command, we're done.
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
        let already = inner.iter().any(|h| {
            h.get("command").and_then(Value::as_str) == Some(command)
                && h.get("type").and_then(Value::as_str).unwrap_or("command") == "command"
        });
        if already {
            return Ok((false, true));
        }
        inner.push(json!({ "type": "command", "command": command }));
        return Ok((true, true));
    }

    // 2. No matching matcher — append a new block.
    arr.push(json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": command }]
    }));
    Ok((true, false))
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
    fn upsert_creates_block_when_empty() {
        let mut hooks = json!({});
        let (added, existed) = upsert_hook(&mut hooks, "PreToolUse", MATCHER, PRE_CMD).unwrap();
        assert!(added);
        assert!(!existed);
        let root = json!({ "hooks": hooks });
        assert_eq!(count_cmds(&root, "PreToolUse", PRE_CMD), 1);
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut hooks = json!({});
        upsert_hook(&mut hooks, "PreToolUse", MATCHER, PRE_CMD).unwrap();
        let (added, existed) = upsert_hook(&mut hooks, "PreToolUse", MATCHER, PRE_CMD).unwrap();
        assert!(!added);
        assert!(existed);
        let root = json!({ "hooks": hooks });
        assert_eq!(count_cmds(&root, "PreToolUse", PRE_CMD), 1);
    }

    #[test]
    fn upsert_appends_command_to_existing_matcher() {
        let mut hooks = json!({
            "PreToolUse": [{
                "matcher": "Edit|Write",
                "hooks": [{ "type": "command", "command": "some-other-tool" }]
            }]
        });
        let (added, existed) = upsert_hook(&mut hooks, "PreToolUse", MATCHER, PRE_CMD).unwrap();
        assert!(added);
        assert!(existed);
        let inner = hooks["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 2);
    }

    #[test]
    fn upsert_appends_new_matcher_block_when_matcher_differs() {
        let mut hooks = json!({
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{ "type": "command", "command": "other" }]
            }]
        });
        let (added, existed) = upsert_hook(&mut hooks, "PreToolUse", MATCHER, PRE_CMD).unwrap();
        assert!(added);
        assert!(!existed);
        let arr = hooks["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn install_at_writes_settings_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".claude/settings.json");
        // First install: creates file and parent dir.
        install_at(&path).unwrap();
        assert!(path.exists());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);
        // Re-install: still exactly one of each.
        install_at(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);
    }

    #[test]
    fn install_preserves_unrelated_top_level_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let path = claude_dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"model":"sonnet","permissions":{"allow":["Bash(ls:*)"]}}"#,
        )
        .unwrap();

        install_at(&path).unwrap();

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["model"], "sonnet");
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls:*)");
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

    /// `--local` builds `<cwd>/.claude/settings.json` regardless of platform —
    /// no env-var lookup, so the shape must hold on Windows too.
    #[test]
    fn settings_path_local_is_cwd_relative() {
        let p = settings_path(true).unwrap();
        let tail: PathBuf = p
            .iter()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .iter()
            .rev()
            .collect();
        assert_eq!(tail, PathBuf::from(".claude").join("settings.json"));
        assert!(
            p.is_absolute(),
            "cwd-derived path should be absolute: {}",
            p.display()
        );
    }

    /// The home-dir branch must not panic on the host platform. We don't
    /// assert the exact prefix (the test runner's `$HOME` / `%USERPROFILE%`
    /// can be anything), only that resolution succeeds *or* fails cleanly
    /// with our error message — never a panic. This is the regression guard
    /// for the Windows bug where `std::env::var("HOME")` silently failed.
    #[test]
    fn settings_path_home_branch_does_not_panic() {
        match settings_path(false) {
            Ok(p) => {
                let tail: PathBuf = p
                    .iter()
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .iter()
                    .rev()
                    .collect();
                assert_eq!(tail, PathBuf::from(".claude").join("settings.json"));
            }
            Err(e) => {
                // The only acceptable failure is the explicit "no home" path,
                // which means `dirs::home_dir()` returned None on this host.
                let msg = e.to_string();
                assert!(
                    msg.contains("home directory"),
                    "unexpected error from settings_path(false): {msg}"
                );
            }
        }
    }
}
