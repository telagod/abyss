//! Integration test for the Codex CLI hook installer.
//!
//! Validates TOML shape, idempotency, preservation of unrelated keys.

use code_abyss::attach::codex;
use toml::Value;

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
fn install_at_writes_valid_toml() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".codex/config.toml");

    codex::install_at(&path).unwrap();
    assert!(path.exists(), "config.toml should be created");

    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = raw.parse().expect("output must be valid TOML");

    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);

    // The hook entries must carry a matcher so the consumer can scope them.
    let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        arr[0].get("matcher").and_then(Value::as_str),
        Some("Edit|Write")
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
    let v: Value = raw.parse().unwrap();
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);
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
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
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
