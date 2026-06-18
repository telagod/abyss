//! Integration test for the OpenClaw hook installer.

use code_abyss::attach::openclaw;
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
    let path = tmp.path().join(".openclaw/config.toml");

    openclaw::install_at(&path).unwrap();
    assert!(path.exists());

    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = raw.parse().unwrap();
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);

    let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(
        arr[0].get("matcher").and_then(Value::as_str),
        Some("Edit|Write")
    );
    assert!(openclaw::already_installed(&path));
}

#[test]
fn install_at_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    for _ in 0..3 {
        openclaw::install_at(&path).unwrap();
    }
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = raw.parse().unwrap();
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);
}

#[test]
fn install_preserves_existing_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, "model = \"opus-4\"\n[persona]\nname = \"邪修\"\n").unwrap();
    openclaw::install_at(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = raw.parse().unwrap();
    assert_eq!(v.get("model").and_then(Value::as_str), Some("opus-4"));
    assert_eq!(
        v.get("persona")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str),
        Some("邪修")
    );
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
}

#[test]
fn settings_path_local_is_cwd_relative() {
    let p = openclaw::settings_path(true).unwrap();
    assert!(p.is_absolute());
    let tail: std::path::PathBuf = p
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .iter()
        .rev()
        .collect();
    assert_eq!(
        tail,
        std::path::PathBuf::from(".openclaw").join("config.toml")
    );
}
