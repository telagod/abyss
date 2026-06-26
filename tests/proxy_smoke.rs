//! Integration smoke tests for `abyss proxy` and `abyss gain`.
//!
//! These tests exercise the CLI entry points end-to-end by spawning the
//! binary and verifying output + exit codes. They don't need a running
//! daemon or a pre-built index (proxy itself doesn't require one; gain
//! gracefully handles an empty tracking table when the DB exists).

use std::process::Command;

fn abyss() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abyss"));
    cmd.env("NO_COLOR", "1");
    cmd
}

#[test]
fn proxy_echo_passthrough() {
    let out = abyss()
        .args(["proxy", "echo", "hello world"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "exit: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello world"), "stdout: {stdout}");
}

#[test]
fn proxy_false_exits_nonzero() {
    let out = abyss()
        .args(["proxy", "false"])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "false should fail");
}

#[test]
fn proxy_explain_prints_handler_info() {
    let out = abyss()
        .args(["proxy", "--explain", "echo", "test"])
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[explain]"), "explain output: {stderr}");
    assert!(stderr.contains("handler:"), "handler info: {stderr}");
    assert!(stderr.contains("tokens"), "token info: {stderr}");
}

#[test]
fn proxy_json_mode() {
    let out = abyss()
        .args(["--json", "proxy", "echo", "json test"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\n{stdout}"));
    assert!(parsed.get("command").is_some(), "has command field");
    assert!(parsed.get("exit_code").is_some(), "has exit_code field");
    assert!(parsed.get("raw_tokens").is_some(), "has raw_tokens field");
}

#[test]
fn proxy_no_command_errors() {
    let out = abyss()
        .args(["proxy"])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "empty proxy should fail");
}

#[test]
fn rewrite_basic_command() {
    let out = abyss()
        .args(["rewrite", "git", "status"])
        .output()
        .expect("spawn");
    assert!(out.status.success(), "rewrite should succeed for git status");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("abyss proxy"), "should rewrite to proxy: {stdout}");
}

#[test]
fn rewrite_unknown_command_exits_1() {
    let out = abyss()
        .args(["rewrite", "some-unknown-program-xyz"])
        .output()
        .expect("spawn");
    assert_eq!(out.status.code(), Some(1), "unknown command should exit 1");
}
