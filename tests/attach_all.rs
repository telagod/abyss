//! Integration test for `abyss attach all`.
//!
//! Runs the binary in a tempdir with `--local`. Three hosts (claude /
//! codex / gemini) get real settings files; openclaw is intentionally
//! skipped with a migration message (v0.5.23 downgrade — OpenClaw uses
//! a per-pack install layout, not a settings file).

use std::path::PathBuf;
use std::process::Command;

fn abyss_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn run_attach_all(cwd: &std::path::Path) -> (String, String, bool) {
    let out = Command::new(abyss_bin())
        .arg("attach")
        .arg("all")
        .arg("--local")
        .current_dir(cwd)
        .output()
        .expect("spawn abyss attach all");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    )
}

#[test]
fn attach_all_local_installs_three_hosts_and_skips_openclaw() {
    let tmp = tempfile::tempdir().unwrap();
    let (stdout, stderr, ok) = run_attach_all(tmp.path());
    assert!(ok, "attach all failed: stdout={stdout} stderr={stderr}");

    // Real installers land their files in --local mode.
    let claude_path = tmp.path().join(".claude/settings.json");
    let codex_path = tmp.path().join(".codex/config.toml");
    let gemini_path = tmp.path().join(".gemini/settings.json");
    let openclaw_path = tmp.path().join(".openclaw/config.toml");

    assert!(claude_path.exists(), "claude settings.json missing");
    assert!(codex_path.exists(), "codex config.toml missing");
    assert!(gemini_path.exists(), "gemini settings.json missing");
    // OpenClaw must NOT have been written — writing a file it doesn't
    // read is worse than a no-op (silent failure).
    assert!(
        !openclaw_path.exists(),
        "openclaw config.toml must not be written in v0.5.23+"
    );

    // Per-host summary line for each host (skipped or installed).
    for host in ["claude", "codex", "gemini", "openclaw"] {
        assert!(
            stdout.contains(host),
            "summary missing host `{host}`: {stdout}"
        );
    }
    assert!(
        stdout.contains("skipped") || stdout.contains("per-pack"),
        "openclaw row must announce the skip: {stdout}"
    );

    // Codex output must use two-level array tables — pin the shape so
    // a future regression to flat `[hooks.X]` is caught here, not in
    // production Codex sessions.
    let codex_raw = std::fs::read_to_string(&codex_path).unwrap();
    assert!(
        codex_raw.contains("[[hooks.SessionStart]]"),
        "codex config must use array-of-tables: {codex_raw}"
    );
    assert!(codex_raw.contains("[[hooks.SessionStart.hooks]]"));
    assert!(codex_raw.contains("[[hooks.PreToolUse]]"));
    assert!(codex_raw.contains("[[hooks.PreToolUse.hooks]]"));
    assert!(codex_raw.contains("[[hooks.PostToolUse]]"));
    assert!(codex_raw.contains("[[hooks.PostToolUse.hooks]]"));

    // Gemini output must use Gemini-native event names.
    let gemini_raw = std::fs::read_to_string(&gemini_path).unwrap();
    let gv: serde_json::Value = serde_json::from_str(&gemini_raw).unwrap();
    assert!(
        gv["hooks"].get("SessionStart").is_some(),
        "gemini SessionStart missing"
    );
    assert!(
        gv["hooks"].get("BeforeTool").is_some(),
        "gemini BeforeTool missing"
    );
    assert!(
        gv["hooks"].get("PreToolUse").is_none(),
        "gemini must NOT carry Claude-shaped PreToolUse: {gv}"
    );

    // Second run: every installed host should report "already present".
    let (stdout2, _stderr2, ok2) = run_attach_all(tmp.path());
    assert!(ok2, "re-run failed: {stdout2}");
    let already_count = stdout2.matches("already present").count();
    assert!(
        already_count >= 3,
        "expected 3+ 'already present' notes on re-run, got {already_count}:\n{stdout2}"
    );
}

#[test]
fn attach_unknown_host_errors_with_list() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(abyss_bin())
        .arg("attach")
        .arg("nonesuch")
        .arg("--local")
        .current_dir(tmp.path())
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "expected non-zero exit for bad host");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown host"),
        "expected 'unknown host' in stderr, got: {stderr}"
    );
    for host in ["claude", "codex", "gemini", "openclaw"] {
        assert!(
            stderr.contains(host),
            "stderr should list `{host}`: {stderr}"
        );
    }
}

#[test]
fn attach_openclaw_directly_returns_clear_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(abyss_bin())
        .arg("attach")
        .arg("openclaw")
        .arg("--local")
        .current_dir(tmp.path())
        .output()
        .expect("spawn");
    assert!(
        !out.status.success(),
        "openclaw direct attach should error to flag the downgrade"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("per-pack") || stderr.contains("npx code-abyss"),
        "openclaw error must surface the migration: {stderr}"
    );
}
