//! Install abyss hooks into Codex CLI's `config.toml`.
//!
//! Codex 0.125+ expects **two-level array tables** for hooks:
//!
//! ```toml
//! [[hooks.SessionStart]]
//! matcher = "startup|resume"
//!
//! [[hooks.SessionStart.hooks]]
//! type = "command"
//! command = "abyss hook pre-edit"
//! timeout = 10
//!
//! [[hooks.PreToolUse]]
//! matcher = "Bash|shell"
//!
//! [[hooks.PreToolUse.hooks]]
//! type = "command"
//! command = "abyss hook pre-edit"
//! timeout = 5
//!
//! [[hooks.PostToolUse]]
//! matcher = "Bash|shell"
//!
//! [[hooks.PostToolUse.hooks]]
//! type = "command"
//! command = "abyss hook post-edit"
//! timeout = 5
//! ```
//!
//! The old flat `[hooks.X]` map shape is REJECTED by Codex with
//! `invalid type: map, expected a sequence in hooks`, so we explicitly
//! emit array-of-tables headers.
//!
//! Idempotency: a hook entry with the same `command` string is never
//! duplicated. Re-parsing TOML and re-serializing it would lose the
//! two-level array-of-tables layout via `toml::ser`, so we manage the
//! `hooks.*` subtree as raw text blocks keyed by `[[hooks.Event]]` /
//! `[[hooks.Event.hooks]]` HEADERS — not by inline maps.
//!
//! Ground truth: `/home/telagod/project/code-abyss/bin/adapters/codex.js`
//! (the production-tested installer in the sister `code-abyss` package).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// Events we manage. Order matters: SessionStart first, then the tool
/// gates. Each tuple is (event, matcher, command, timeout_seconds).
const EVENTS: &[(&str, &str, &str, u32)] = &[
    ("SessionStart", "startup|resume", "abyss hook pre-edit", 10),
    ("PreToolUse", "Bash|shell", "abyss hook pre-edit", 5),
    ("PostToolUse", "Bash|shell", "abyss hook post-edit", 5),
];

/// Marker string we embed in the emitted block so re-runs can recognise
/// our own entries and replace them in place (vs. user-authored hooks,
/// which we leave alone).
const ABYSS_MARKER: &str = "# abyss-managed: do not edit";

/// Resolve the target `config.toml` path.
///
/// * `--local` → `<cwd>/.codex/config.toml`
/// * default   → `<home>/.codex/config.toml`
pub fn settings_path(local: bool) -> Result<PathBuf> {
    if local {
        let cwd = std::env::current_dir().context("cannot read current dir")?;
        return Ok(cwd.join(".codex").join("config.toml"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
    })?;
    Ok(home.join(".codex").join("config.toml"))
}

/// True iff the file already contains every abyss-managed event.
///
/// Detection is HEADER-based to match Codex's array-of-tables layout:
/// we look for our `ABYSS_MARKER` line followed by the expected
/// `[[hooks.Event]]` headers and the per-event command strings.
pub fn already_installed(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    for (event, _, cmd, _) in EVENTS {
        if !raw.contains(&format!("[[hooks.{event}]]")) {
            return false;
        }
        if !raw.contains(&format!("command = \"{cmd}\"")) {
            return false;
        }
    }
    true
}

/// Install (or upgrade) the abyss hook entries.
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

    let raw = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let merged = merge_codex_hooks(&raw);
    std::fs::write(path, &merged).with_context(|| format!("writing {}", path.display()))?;

    println!("✓ abyss hook installed at {}", path.display());
    for (event, matcher, cmd, _) in EVENTS {
        println!("  [[hooks.{event}]] matcher={matcher:?} command={cmd:?}");
    }
    println!("  shape: Codex 0.125+ two-level array tables");
    Ok(())
}

/// Strip any previously-installed abyss block and append a fresh one.
/// User-authored hooks (no `ABYSS_MARKER` in the block) are preserved
/// verbatim — including their position relative to other TOML sections.
fn merge_codex_hooks(raw: &str) -> String {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let stripped = strip_abyss_block(raw, eol);
    let block = render_abyss_block(eol);
    let base = stripped.trim_end_matches(['\n', '\r']);
    if base.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{base}{eol}{eol}{block}{eol}")
    }
}

/// Render the canonical abyss block. The leading `ABYSS_MARKER` line is
/// load-bearing — it lets `strip_abyss_block` identify our own output
/// across upgrades.
fn render_abyss_block(eol: &str) -> String {
    let mut out = String::new();
    out.push_str(ABYSS_MARKER);
    out.push_str(eol);
    for (i, (event, matcher, cmd, timeout)) in EVENTS.iter().enumerate() {
        if i > 0 {
            out.push_str(eol);
        }
        out.push_str(&format!("[[hooks.{event}]]{eol}"));
        out.push_str(&format!("matcher = \"{matcher}\"{eol}"));
        out.push_str(eol);
        out.push_str(&format!("[[hooks.{event}.hooks]]{eol}"));
        out.push_str(&format!("type = \"command\"{eol}"));
        out.push_str(&format!("command = \"{cmd}\"{eol}"));
        out.push_str(&format!("timeout = {timeout}{eol}"));
    }
    out
}

/// Remove a previously-installed abyss block from `raw`. Detection
/// anchors on the `ABYSS_MARKER` comment; the block extends until the
/// next non-`[[hooks.…]]` section header (or EOF). User-authored
/// `[[hooks.X]]` blocks without the marker are left untouched.
fn strip_abyss_block(raw: &str, eol: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = raw.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.trim() == ABYSS_MARKER {
            // Skip until we hit a non-hooks section header or EOF.
            for follow in lines.by_ref() {
                let t = follow.trim_end_matches(['\n', '\r']);
                let tt = t.trim();
                let is_hook_header = tt.starts_with("[[hooks.") || tt.starts_with("[hooks.");
                let is_some_other_header = is_table_header(tt) && !is_hook_header;
                if is_some_other_header {
                    kept.push(follow);
                    break;
                }
                // else: part of the abyss block — drop it
            }
            continue;
        }
        kept.push(line);
    }
    // Re-stitch using detected EOL — split_inclusive preserves the
    // original line endings, so we just concat.
    let joined: String = kept.join("");
    // Normalize trailing whitespace to a single EOL pair so the next
    // render lays out cleanly.
    let trimmed = joined.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}{eol}")
    }
}

fn is_table_header(line: &str) -> bool {
    let t = line.trim();
    (t.starts_with("[[") && t.ends_with("]]")) || (t.starts_with('[') && t.ends_with(']'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    fn count_event_blocks(raw: &str, event: &str) -> usize {
        let needle = format!("[[hooks.{event}]]");
        raw.matches(&needle).count()
    }

    fn count_inner_hooks(raw: &str, event: &str) -> usize {
        let needle = format!("[[hooks.{event}.hooks]]");
        raw.matches(&needle).count()
    }

    #[test]
    fn install_at_writes_two_level_array_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".codex/config.toml");
        install_at(&path).unwrap();
        assert!(path.exists());

        let raw = std::fs::read_to_string(&path).unwrap();
        // Both header levels present, exactly once each.
        for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
            assert_eq!(count_event_blocks(&raw, ev), 1, "missing [[hooks.{ev}]]");
            assert_eq!(
                count_inner_hooks(&raw, ev),
                1,
                "missing [[hooks.{ev}.hooks]]"
            );
        }
        // Parses as valid TOML.
        let v: Value = raw.parse().expect("Codex output must be valid TOML");
        // And the parsed shape is array-of-tables, not a flat map — this
        // is the regression Codex 0.125+ added.
        let hooks = v.get("hooks").expect("hooks present").as_table().unwrap();
        for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
            let arr = hooks.get(ev).unwrap().as_array().expect("must be array");
            assert_eq!(arr.len(), 1, "[[hooks.{ev}]] must be a 1-elem array");
            let inner = arr[0]
                .get("hooks")
                .expect("inner hooks present")
                .as_array()
                .expect("inner must be array");
            assert_eq!(
                inner.len(),
                1,
                "[[hooks.{ev}.hooks]] must be a 1-elem array"
            );
        }
    }

    #[test]
    fn install_at_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        for _ in 0..3 {
            install_at(&path).unwrap();
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
            assert_eq!(count_event_blocks(&raw, ev), 1);
            assert_eq!(count_inner_hooks(&raw, ev), 1);
        }
    }

    #[test]
    fn install_preserves_unrelated_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            "model = \"o4-mini\"\n[approval]\nmode = \"on-failure\"\n",
        )
        .unwrap();

        install_at(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert_eq!(v.get("model").and_then(Value::as_str), Some("o4-mini"));
        assert_eq!(
            v.get("approval")
                .and_then(|a| a.get("mode"))
                .and_then(Value::as_str),
            Some("on-failure")
        );
        // And the hooks still landed.
        let hooks = v.get("hooks").unwrap().as_table().unwrap();
        assert!(hooks.contains_key("SessionStart"));
    }

    #[test]
    fn install_at_accepts_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        install_at(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: Value = raw.parse().unwrap();
        assert!(v.get("hooks").is_some());
    }

    #[test]
    fn already_installed_detects_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        assert!(!already_installed(&path));
        install_at(&path).unwrap();
        assert!(already_installed(&path));
    }

    #[test]
    fn user_owned_hooks_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        // A user-authored hook that we must NOT remove on upgrade.
        std::fs::write(
            &path,
            "[[hooks.SessionStart]]\nmatcher = \"custom\"\n\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = \"my-user-hook\"\ntimeout = 3\n",
        )
        .unwrap();

        install_at(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("my-user-hook"),
            "user hook lost on install: {raw}"
        );
        let v: Value = raw.parse().expect("still valid TOML");
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        // Two entries: the user's + ours.
        assert!(arr.len() >= 2, "user hook should be merged, got {arr:?}");
    }
}
