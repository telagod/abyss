//! Integration test for the Gemini CLI hook installer.
//!
//! Validates the **Gemini-native** event names — `SessionStart`,
//! `BeforeTool`, `AfterTool` — and the matchers / hook-entry shape
//! mandated by the sister `code-abyss` package's
//! `injectGeminiHooks` (ground truth).

use code_abyss::attach::gemini;
use serde_json::Value;

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
fn install_at_uses_gemini_native_events() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".gemini/settings.json");

    gemini::install_at(&path).unwrap();
    assert!(path.exists());
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    // SessionStart / BeforeTool / AfterTool — NOT PreToolUse / PostToolUse
    // (those are Claude-only event names, the wrong shape for Gemini).
    for ev in ["SessionStart", "BeforeTool", "AfterTool"] {
        assert!(
            v["hooks"].get(ev).is_some(),
            "missing Gemini event {ev}: {v}"
        );
    }
    assert!(
        v["hooks"].get("PreToolUse").is_none(),
        "PreToolUse is a Claude event; must not appear in Gemini settings"
    );
    assert!(v["hooks"].get("PostToolUse").is_none());

    // Matchers per ground truth.
    assert_eq!(v["hooks"]["SessionStart"][0]["matcher"], "startup");
    assert_eq!(
        v["hooks"]["BeforeTool"][0]["matcher"],
        "write_file|replace|edit_file"
    );
    assert_eq!(
        v["hooks"]["AfterTool"][0]["matcher"],
        "write_file|replace|edit_file"
    );

    // Hook entries are { name, type, command, timeout(ms), description }.
    // Timeout in MILLISECONDS for Gemini (Codex uses seconds — don't mix).
    let init = find_named(&v, "SessionStart", "abyss-init").expect("abyss-init");
    assert_eq!(init["type"], "command");
    assert_eq!(init["command"], "abyss hook pre-edit");
    assert_eq!(init["timeout"], 10_000);
    assert!(init.get("description").is_some());

    let check = find_named(&v, "BeforeTool", "abyss-check").expect("abyss-check");
    assert_eq!(check["timeout"], 5_000);
    assert_eq!(check["command"], "abyss hook pre-edit");

    let post = find_named(&v, "AfterTool", "abyss-post").expect("abyss-post");
    assert_eq!(post["timeout"], 5_000);
    assert_eq!(post["command"], "abyss hook post-edit");

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
    for ev in ["SessionStart", "BeforeTool", "AfterTool"] {
        let arr = v["hooks"][ev].as_array().unwrap();
        assert_eq!(arr.len(), 1, "duplicate matcher block for {ev}");
        assert_eq!(
            arr[0]["hooks"].as_array().unwrap().len(),
            1,
            "duplicate named hook in {ev}"
        );
    }
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
    assert!(find_named(&v, "BeforeTool", "abyss-check").is_some());
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
