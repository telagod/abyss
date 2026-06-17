//! `abyss reset` CLI contract.
//!
//! Pins three operator promises so a refactor can't silently change scope:
//!   * default (`abyss reset`) removes `index.db` only, preserves `arch.toml`
//!   * `--all` removes the entire `.code-abyss/` directory
//!   * `--dry-run` mutates nothing, even when paths exist
//!
//! Linux-only — the daemon-liveness probe uses `kill(pid, 0)` semantics
//! and we want a deterministic CI lane.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

/// Seed `.code-abyss/` with the surfaces the reset scopes touch:
/// `index.db`, `arch.toml`, plus the three daemon artifacts. Returns the
/// canonicalized workspace so the binary's `--workspace` arg matches what
/// the in-process `Config::new` would resolve.
fn seed_workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");
    let abyss = ws.join(".code-abyss");
    std::fs::create_dir_all(&abyss).expect("mkdir .code-abyss");
    std::fs::write(abyss.join("index.db"), b"fake-db").expect("write index.db");
    std::fs::write(abyss.join("arch.toml"), b"[layer]\n").expect("write arch.toml");
    std::fs::write(abyss.join("daemon.pid"), b"99999\n").expect("write daemon.pid");
    std::fs::write(abyss.join("daemon.sock"), b"").expect("write daemon.sock");
    std::fs::write(abyss.join("daemon.log"), b"log\n").expect("write daemon.log");
    (dir, ws)
}

#[test]
fn default_reset_removes_index_db_only() {
    let (_guard, ws) = seed_workspace();
    let abyss = ws.join(".code-abyss");

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("reset")
        .output()
        .expect("spawn abyss reset");
    assert!(
        out.status.success(),
        "reset failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    // index.db gone; arch.toml + daemon files preserved.
    assert!(
        !abyss.join("index.db").exists(),
        "default reset must remove index.db"
    );
    assert!(
        abyss.join("arch.toml").exists(),
        "default reset must preserve arch.toml (user overrides)"
    );
    assert!(
        abyss.join("daemon.log").exists(),
        "default reset must preserve daemon.log (scope flag controls daemon files)"
    );
}

#[test]
fn reset_all_wipes_the_whole_directory() {
    let (_guard, ws) = seed_workspace();
    let abyss = ws.join(".code-abyss");

    // Stale daemon.pid points at a pid we won't reach. The liveness probe
    // must not refuse — a stale pidfile is the normal post-crash state and
    // reset is exactly the cleanup tool for it.
    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("reset")
        .arg("--all")
        .output()
        .expect("spawn abyss reset --all");
    assert!(
        out.status.success(),
        "reset --all failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !abyss.exists(),
        "reset --all must remove .code-abyss/ entirely"
    );
}

#[test]
fn dry_run_mutates_nothing() {
    let (_guard, ws) = seed_workspace();
    let abyss = ws.join(".code-abyss");

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("reset")
        .arg("--all")
        .arg("--dry-run")
        .output()
        .expect("spawn abyss reset --all --dry-run");
    assert!(out.status.success(), "dry-run should succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("would remove"),
        "dry-run must announce the targets via 'would remove': {stdout}"
    );
    // Every seed file still present.
    for name in [
        "index.db",
        "arch.toml",
        "daemon.pid",
        "daemon.sock",
        "daemon.log",
    ] {
        assert!(
            abyss.join(name).exists(),
            "dry-run mutated {name} — should be a no-op"
        );
    }
}

#[test]
fn daemon_scope_removes_only_daemon_artifacts() {
    let (_guard, ws) = seed_workspace();
    let abyss = ws.join(".code-abyss");

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("reset")
        .arg("--daemon")
        .output()
        .expect("spawn abyss reset --daemon");
    assert!(out.status.success(), "reset --daemon should succeed");

    assert!(!abyss.join("daemon.pid").exists());
    assert!(!abyss.join("daemon.sock").exists());
    assert!(!abyss.join("daemon.log").exists());
    // DB and arch.toml untouched.
    assert!(abyss.join("index.db").exists());
    assert!(abyss.join("arch.toml").exists());
}
