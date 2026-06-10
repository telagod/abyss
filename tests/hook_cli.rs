//! CLI-level hook tests: run the actual binary the way an agent hook would —
//! tool-call JSON on stdin, warnings expected on stderr, exit code always 0.

mod common;
use std::io::Write;
use std::process::{Command, Stdio};

use common::*;

fn run_hook(fx: &Fixture, args: &[&str], stdin_json: &str) -> (String, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_abyss"))
        .arg("--workspace")
        .arg(&fx.config.workspace)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_json.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    )
}

fn edited_file_fixture() -> Fixture {
    index_fixture(&[
        (
            "app/core.go",
            "package app\n\nfunc Target() int { return 1 }\n",
        ),
        (
            "app/user.go",
            "package app\n\nfunc Caller() int { return Target() }\n",
        ),
    ])
}

#[test]
fn pre_edit_warns_about_production_callers() {
    let fx = edited_file_fixture();
    let abs = fx.config.workspace.join("app/core.go");
    let payload = format!(r#"{{"tool_input": {{"file_path": "{}"}}}}"#, abs.display());

    let (_, stderr, ok) = run_hook(&fx, &["hook", "pre-edit"], &payload);
    assert!(ok);
    assert!(
        stderr.contains("core.go") && stderr.contains("1 production caller"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Target"), "stderr: {stderr}");
}

#[test]
fn pre_edit_json_emits_full_context_on_stdout() {
    let fx = edited_file_fixture();
    let payload = r#"{"file_path": "app/core.go"}"#;

    let (stdout, _, ok) = run_hook(&fx, &["hook", "pre-edit", "--json"], payload);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["file"], "app/core.go");
    assert_eq!(v["symbols_with_external_callers"][0]["symbol"], "Target");
}

#[test]
fn pre_edit_is_silent_for_irrelevant_input() {
    let fx = edited_file_fixture();
    for payload in [
        r#"{"command": "ls -la"}"#,            // no file path
        r#"{"file_path": "README.md"}"#,       // unsupported language
        r#"{"file_path": "no/such/file.go"}"#, // not indexed
        "not json at all",
    ] {
        let (stdout, stderr, ok) = run_hook(&fx, &["hook", "pre-edit"], payload);
        assert!(ok, "payload: {payload}");
        assert_eq!(stdout, "", "payload: {payload}");
        assert_eq!(stderr, "", "payload: {payload}, stderr: {stderr}");
    }
}

#[test]
fn pre_edit_sees_brand_new_callers_after_refresh() {
    // A caller added AFTER the last index run must still be reported:
    // the hook refreshes incrementally before querying.
    let fx = edited_file_fixture();
    write_files(
        &fx.config.workspace,
        &[(
            "app/late.go",
            "package app\n\nfunc LateCaller() int { return Target() }\n",
        )],
    );
    let payload = r#"{"file_path": "app/core.go"}"#;
    let (_, stderr, ok) = run_hook(&fx, &["hook", "pre-edit"], payload);
    assert!(ok);
    assert!(
        stderr.contains("2 production caller"),
        "stale index — refresh did not pick up late.go: {stderr}"
    );
}

#[test]
fn post_edit_refreshes_the_index() {
    let fx = edited_file_fixture();
    write_files(
        &fx.config.workspace,
        &[(
            "app/new.go",
            "package app\n\nfunc Fresh() int { return Target() }\n",
        )],
    );
    let (_, _, ok) = run_hook(&fx, &["hook", "post-edit"], "{}");
    assert!(ok);
    // Index must now contain the new caller.
    let refs = call_refs_to(&fx.repo, "Target");
    assert_eq!(refs.len(), 2, "{refs:?}");
}

#[test]
fn hook_without_index_is_a_silent_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(ws.join("a.go"), "package a\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_abyss"))
        .arg("--workspace")
        .arg(&ws)
        .args(["hook", "pre-edit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(br#"{"file_path": "a.go"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}
