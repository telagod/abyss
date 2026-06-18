//! Integration test for the Gemini CLI hook installer.
//!
//! Mirrors Claude's settings.json shape; here we validate that the JSON
//! parses, the hook commands land under the expected event keys, and
//! re-running is a no-op.

use code_abyss::attach::gemini;
use serde_json::Value;

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
fn install_at_writes_valid_json() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".gemini/settings.json");

    gemini::install_at(&path).unwrap();
    assert!(path.exists());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);

    let entry = &v["hooks"]["PreToolUse"][0];
    assert_eq!(entry["matcher"], "Edit|Write");
    assert!(gemini::already_installed(&path));
}

#[test]
fn install_at_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    for _ in 0..3 {
        gemini::install_at(&path).unwrap();
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
    assert_eq!(count_cmds(&v, "PostToolUse", "abyss hook post-edit"), 1);
}

#[test]
fn install_preserves_existing_config() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"theme":"dark","model":"gemini-2.5-pro","other":{"keep":"me"}}"#,
    )
    .unwrap();
    gemini::install_at(&path).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["theme"], "dark");
    assert_eq!(v["model"], "gemini-2.5-pro");
    assert_eq!(v["other"]["keep"], "me");
    assert_eq!(count_cmds(&v, "PreToolUse", "abyss hook pre-edit"), 1);
}

#[test]
fn settings_path_local_is_cwd_relative() {
    let p = gemini::settings_path(true).unwrap();
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
        std::path::PathBuf::from(".gemini").join("settings.json")
    );
}
