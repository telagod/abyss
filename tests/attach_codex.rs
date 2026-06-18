//! Integration test for the Codex CLI hook installer.
//!
//! Validates the **two-level array-of-tables** layout that Codex 0.125+
//! requires (`[[hooks.Event]]` + `[[hooks.Event.hooks]]`), TOML
//! parseability, idempotency, and preservation of unrelated keys.

use code_abyss::attach::codex;
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
fn install_at_writes_valid_codex_125_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".codex/config.toml");

    codex::install_at(&path).unwrap();
    assert!(path.exists(), "config.toml should be created");

    let raw = std::fs::read_to_string(&path).unwrap();

    // Two-level array tables for every event we manage. The old flat
    // `[hooks.X]` shape is REJECTED by Codex 0.125+ with
    // `invalid type: map, expected a sequence in hooks`.
    for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
        assert_eq!(
            count_event_blocks(&raw, ev),
            1,
            "missing [[hooks.{ev}]] header in:\n{raw}"
        );
        assert_eq!(
            count_inner_hooks(&raw, ev),
            1,
            "missing [[hooks.{ev}.hooks]] header in:\n{raw}"
        );
    }

    let v: Value = raw.parse().expect("output must be valid TOML");

    // Codex 0.125+: hooks.<event> must be a *sequence*, not a map.
    let hooks = v.get("hooks").unwrap().as_table().unwrap();
    for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
        let arr = hooks
            .get(ev)
            .unwrap_or_else(|| panic!("hooks.{ev} missing"))
            .as_array()
            .expect("hooks.<event> must be array-of-tables");
        assert_eq!(arr.len(), 1);
        let inner = arr[0]
            .get("hooks")
            .expect("inner hooks present")
            .as_array()
            .expect("inner hooks must be array-of-tables too");
        assert_eq!(inner.len(), 1);
        // Inner entries carry type/command/timeout.
        assert_eq!(
            inner[0].get("type").and_then(Value::as_str),
            Some("command")
        );
        assert!(inner[0].get("command").and_then(Value::as_str).is_some());
        assert!(
            inner[0]
                .get("timeout")
                .and_then(Value::as_integer)
                .is_some()
        );
    }

    // Matchers per ground truth (code-abyss/bin/adapters/codex.js).
    assert_eq!(
        v["hooks"]["SessionStart"][0]["matcher"].as_str(),
        Some("startup|resume")
    );
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["matcher"].as_str(),
        Some("Bash|shell")
    );
    assert_eq!(
        v["hooks"]["PostToolUse"][0]["matcher"].as_str(),
        Some("Bash|shell")
    );

    // Pre/post commands wired correctly.
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str(),
        Some("abyss hook pre-edit")
    );
    assert_eq!(
        v["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str(),
        Some("abyss hook post-edit")
    );

    assert!(codex::already_installed(&path));
}

#[test]
fn install_at_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    codex::install_at(&path).unwrap();
    codex::install_at(&path).unwrap();
    codex::install_at(&path).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    for ev in ["SessionStart", "PreToolUse", "PostToolUse"] {
        assert_eq!(count_event_blocks(&raw, ev), 1);
        assert_eq!(count_inner_hooks(&raw, ev), 1);
    }
}

#[test]
fn install_preserves_existing_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
model = "o4-mini"

[approval]
mode = "on-failure"
"#,
    )
    .unwrap();

    codex::install_at(&path).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = raw.parse().unwrap();
    assert_eq!(v.get("model").and_then(Value::as_str), Some("o4-mini"));
    assert_eq!(
        v.get("approval")
            .and_then(|a| a.get("mode"))
            .and_then(Value::as_str),
        Some("on-failure")
    );
    // Hooks still installed.
    assert!(v["hooks"]["SessionStart"].is_array());
}

#[test]
fn settings_path_local_is_cwd_relative() {
    let p = codex::settings_path(true).unwrap();
    assert!(p.is_absolute());
    let tail: std::path::PathBuf = p
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .iter()
        .rev()
        .collect();
    assert_eq!(tail, std::path::PathBuf::from(".codex").join("config.toml"));
}
