//! Integration test for `abyss attach all`.
//!
//! Runs the binary in a tempdir with `--local`, so every host's settings
//! file is created under `<tempdir>/.<host>/`. Re-runs are idempotent
//! and emit "already present" notes per host.

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
fn attach_all_local_installs_every_host() {
    let tmp = tempfile::tempdir().unwrap();
    let (stdout, stderr, ok) = run_attach_all(tmp.path());
    assert!(ok, "attach all failed: stdout={stdout} stderr={stderr}");

    // Each host should land its settings file in --local mode.
    let claude_path = tmp.path().join(".claude/settings.json");
    let codex_path = tmp.path().join(".codex/config.toml");
    let gemini_path = tmp.path().join(".gemini/settings.json");
    let openclaw_path = tmp.path().join(".openclaw/config.toml");

    assert!(claude_path.exists(), "claude settings.json missing");
    assert!(codex_path.exists(), "codex config.toml missing");
    assert!(gemini_path.exists(), "gemini settings.json missing");
    assert!(openclaw_path.exists(), "openclaw config.toml missing");

    // Per-host summary line for each host.
    for host in ["claude", "codex", "gemini", "openclaw"] {
        assert!(
            stdout.contains(host),
            "summary missing host `{host}`: {stdout}"
        );
    }

    // Second run: every host should report "already present".
    let (stdout2, _stderr2, ok2) = run_attach_all(tmp.path());
    assert!(ok2, "re-run failed: {stdout2}");
    let already_count = stdout2.matches("already present").count();
    assert!(
        already_count >= 4,
        "expected 4+ 'already present' notes on re-run, got {already_count}:\n{stdout2}"
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
    // List the known hosts in the error so the user knows what to try.
    for host in ["claude", "codex", "gemini", "openclaw"] {
        assert!(
            stderr.contains(host),
            "stderr should list `{host}`: {stderr}"
        );
    }
}
