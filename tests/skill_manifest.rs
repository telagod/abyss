//! Integration test for `abyss skill-manifest`.
//!
//! Drives the binary, parses stdout as JSON, asserts the contract that
//! the companion `code-abyss` skill-discovery consumer relies on.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn abyss_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn run_manifest(extra_args: &[&str]) -> (String, String, bool) {
    let out = Command::new(abyss_bin())
        .arg("skill-manifest")
        .args(extra_args)
        .output()
        .expect("spawn abyss skill-manifest");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    )
}

#[test]
fn skill_manifest_emits_valid_json_with_required_keys() {
    let (stdout, stderr, ok) = run_manifest(&[]);
    assert!(ok, "skill-manifest failed: stderr={stderr}");
    let v: Value = serde_json::from_str(&stdout).expect("skill-manifest stdout must parse as JSON");

    assert_eq!(v["name"].as_str(), Some("abyss"));
    assert_eq!(v["kind"].as_str(), Some("code-graph"));
    assert_eq!(v["schema_version"].as_u64(), Some(1));
    assert!(v["version"].as_str().is_some());
    assert!(v["description"].as_str().is_some());
    assert!(v["homepage"].as_str().is_some());
    assert!(v["repo"].as_str().is_some());

    // providers.cli.commands non-empty
    let cmds = v["providers"]["cli"]["commands"]
        .as_array()
        .expect("cli.commands must be array");
    assert!(!cmds.is_empty(), "expected non-empty cli commands");
    for c in cmds {
        assert!(c["name"].as_str().is_some(), "command missing name: {c}");
        assert!(
            c["summary"].as_str().is_some(),
            "command missing summary: {c}"
        );
    }

    // providers.mcp.tools length >= 7
    let tools = v["providers"]["mcp"]["tools"]
        .as_array()
        .expect("mcp.tools must be array");
    assert!(
        tools.len() >= 7,
        "expected >=7 MCP tools, got {}",
        tools.len()
    );

    // providers.hooks.attach must list all four hosts.
    let attach = &v["providers"]["hooks"]["attach"];
    for host in ["claude", "codex", "gemini", "openclaw", "all"] {
        assert!(
            attach.get(host).and_then(Value::as_str).is_some(),
            "attach hosts missing: {host}"
        );
    }
    assert_eq!(
        v["providers"]["hooks"]["pre_edit"].as_str(),
        Some("abyss hook pre-edit")
    );

    // daemon socket + verbs
    let verbs = v["providers"]["daemon"]["verbs"]
        .as_array()
        .expect("daemon.verbs must be array");
    assert!(verbs.iter().any(|x| x.as_str() == Some("ping")));
    assert!(verbs.iter().any(|x| x.as_str() == Some("mcp")));
}

#[test]
fn skill_manifest_compact_emits_single_line() {
    let (stdout, _stderr, ok) = run_manifest(&["--compact"]);
    assert!(ok);
    // Trim only the trailing newline added by println!.
    let trimmed = stdout.trim_end_matches('\n');
    assert!(
        !trimmed.contains('\n'),
        "--compact should be single-line, got: {trimmed:?}"
    );
    // Still parses as JSON.
    let _: Value = serde_json::from_str(trimmed).expect("single-line output must be valid JSON");
}

#[test]
fn skill_manifest_version_matches_binary() {
    let (stdout, _stderr, ok) = run_manifest(&[]);
    assert!(ok);
    let v: Value = serde_json::from_str(&stdout).unwrap();
    let manifest_version = v["version"].as_str().unwrap();
    assert!(
        manifest_version.starts_with(char::is_numeric),
        "version should be semver-shaped: {manifest_version}"
    );
}
