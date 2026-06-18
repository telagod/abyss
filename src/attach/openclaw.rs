//! Install abyss hooks into OpenClaw's `config.toml`.
//!
//! Layout (only touches the `[hooks]` subtree, leaves the rest alone):
//!
//! ```toml
//! [[hooks.PreToolUse]]
//! matcher = "Edit|Write"
//! command = "abyss hook pre-edit"
//!
//! [[hooks.PostToolUse]]
//! matcher = "Edit|Write"
//! command = "abyss hook post-edit"
//! ```
//!
//! Idempotent: a hook entry with the same `command` string is never
//! duplicated. Best-effort schema — OpenClaw's hook config layout is
//! still maturing. If your version disagrees, please file an issue at
//! <https://github.com/telagod/abyss/issues>.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use toml::Value;
use toml::value::Table;

const MATCHER: &str = "Edit|Write";
const PRE_CMD: &str = "abyss hook pre-edit";
const POST_CMD: &str = "abyss hook post-edit";

/// Resolve the target `config.toml` path.
///
/// * `--local` → `<cwd>/.openclaw/config.toml`
/// * default   → `<home>/.openclaw/config.toml`
pub fn settings_path(local: bool) -> Result<PathBuf> {
    if local {
        let cwd = std::env::current_dir().context("cannot read current dir")?;
        return Ok(cwd.join(".openclaw").join("config.toml"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
    })?;
    Ok(home.join(".openclaw").join("config.toml"))
}

pub fn already_installed(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = raw.parse::<Value>() else {
        return false;
    };
    has_command(&value, "PreToolUse", PRE_CMD) && has_command(&value, "PostToolUse", POST_CMD)
}

fn has_command(root: &Value, event: &str, cmd: &str) -> bool {
    root.get("hooks")
        .and_then(|h| h.get(event))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .any(|e| e.get("command").and_then(Value::as_str) == Some(cmd))
        })
        .unwrap_or(false)
}

pub fn install(local: bool) -> Result<()> {
    let path = settings_path(local)?;
    install_at(&path)
}

/// Test-friendly variant: install into an explicit `config.toml` path.
pub fn install_at(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut root: Value = if path.exists() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        if raw.trim().is_empty() {
            Value::Table(Table::new())
        } else {
            raw.parse::<Value>()
                .with_context(|| format!("parsing {} as TOML", path.display()))?
        }
    } else {
        Value::Table(Table::new())
    };

    if !root.is_table() {
        anyhow::bail!(
            "{} is not a TOML table — refusing to overwrite",
            path.display()
        );
    }

    let pre_added = upsert_hook(&mut root, "PreToolUse", MATCHER, PRE_CMD)?;
    let post_added = upsert_hook(&mut root, "PostToolUse", MATCHER, POST_CMD)?;

    let serialized = toml::to_string_pretty(&root)?;
    std::fs::write(path, serialized).with_context(|| format!("writing {}", path.display()))?;

    println!("✓ abyss hook installed at {}", path.display());
    println!(
        "  [hooks.PreToolUse]  ({}): {PRE_CMD}",
        if pre_added {
            "added"
        } else {
            "already present"
        }
    );
    println!(
        "  [hooks.PostToolUse] ({}): {POST_CMD}",
        if post_added {
            "added"
        } else {
            "already present"
        }
    );
    println!("  note: OpenClaw hook schema is evolving — if this layout doesn't");
    println!("        match your version, please file an issue.");
    Ok(())
}

fn upsert_hook(root: &mut Value, event: &str, matcher: &str, command: &str) -> Result<bool> {
    let root_table = root
        .as_table_mut()
        .ok_or_else(|| anyhow!("root is not a TOML table"))?;

    let hooks_entry = root_table
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Table(Table::new()));
    if !hooks_entry.is_table() {
        anyhow::bail!("`hooks` field exists but is not a TOML table");
    }
    let hooks_table = hooks_entry.as_table_mut().expect("checked");

    let arr_entry = hooks_table
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !arr_entry.is_array() {
        anyhow::bail!("`hooks.{event}` exists but is not a TOML array");
    }
    let arr = arr_entry.as_array_mut().expect("checked");

    let already = arr.iter().any(|entry| {
        entry.get("command").and_then(Value::as_str) == Some(command)
            && entry.get("matcher").and_then(Value::as_str).unwrap_or("") == matcher
    });
    if already {
        return Ok(false);
    }

    let mut entry = Table::new();
    entry.insert("matcher".to_string(), Value::String(matcher.to_string()));
    entry.insert("command".to_string(), Value::String(command.to_string()));
    arr.push(Value::Table(entry));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_cmds(root: &Value, event: &str, cmd: &str) -> usize {
        root.get("hooks")
            .and_then(|h| h.get(event))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|e| e.get("command").and_then(Value::as_str) == Some(cmd))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn install_at_writes_settings_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".openclaw/config.toml");
        install_at(&path).unwrap();
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);

        install_at(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
        assert_eq!(count_cmds(&v, "PostToolUse", POST_CMD), 1);
    }

    #[test]
    fn install_preserves_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "model = \"opus-4\"\n[persona]\nname = \"邪修\"\n").unwrap();

        install_at(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert_eq!(v.get("model").and_then(Value::as_str), Some("opus-4"));
        assert_eq!(
            v.get("persona")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str),
            Some("邪修")
        );
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
    }

    #[test]
    fn install_at_accepts_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        install_at(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert_eq!(count_cmds(&v, "PreToolUse", PRE_CMD), 1);
    }

    #[test]
    fn already_installed_detects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        assert!(!already_installed(&path));
        install_at(&path).unwrap();
        assert!(already_installed(&path));
    }
}
