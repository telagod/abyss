//! `abyss config show` smoke test.
//!
//! Indexes a tiny tempdir workspace, then runs the new subcommand in JSON
//! mode and asserts the contract:
//!   * top-level keys (`workspace`, `db_path`, `schema_version`,
//!     `index_present`, `counts`, `arch_toml`, `dictionary`, `daemon`)
//!   * after an index pass, `index_present` is true, `schema_version >= 1`,
//!     `counts.files >= 1`
//!   * before any daemon ran, `daemon.running == false`
//!
//! Linux-only — keeps the daemon-state branch deterministic (`/proc`-backed
//! `kill(pid,0)` semantics). The functional shape would work on macOS too;
//! we just want a stable CI lane.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

#[test]
fn config_show_json_after_index_returns_valid_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");

    // Seed a tiny indexable file so the structural pass has real counts.
    std::fs::write(ws.join("main.go"), "package main\n\nfunc Alpha() {}\n").expect("write main.go");
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).expect("open repo");
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).expect("structural index");
    }

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("--json")
        .arg("config")
        .arg("show")
        .output()
        .expect("spawn abyss config show");
    assert!(
        out.status.success(),
        "config show exit nonzero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("invalid JSON: {stdout}"));

    // Required keys — pin the envelope so a rename gets caught here, not
    // by some downstream agent silently dropping a field.
    for key in [
        "workspace",
        "db_path",
        "log_path",
        "schema_version",
        "index_present",
        "arch_toml",
        "dictionary",
        "counts",
        "arch_layer_counts",
        "daemon",
    ] {
        assert!(
            v.get(key).is_some(),
            "missing key `{key}` in config show JSON:\n{stdout}"
        );
    }

    // Workspace path must be the canonicalized tempdir we passed in.
    assert_eq!(
        v["workspace"].as_str().unwrap_or(""),
        ws.display().to_string()
    );

    // Schema version is set on every index pass — must be a positive int.
    assert!(
        v["schema_version"].as_i64().unwrap_or(0) >= 1,
        "schema_version not bumped: {stdout}"
    );

    assert_eq!(
        v["index_present"].as_bool(),
        Some(true),
        "index_present should be true after running index pass: {stdout}"
    );

    // We indexed exactly one Go file — counts.files must reflect it.
    let files = v["counts"]["files"].as_i64().unwrap_or(0);
    assert!(
        files >= 1,
        "counts.files should be >= 1 after indexing main.go, got {files}: {stdout}"
    );

    // arch.toml absent → present:false.
    assert_eq!(
        v["arch_toml"]["present"].as_bool(),
        Some(false),
        "arch.toml should be absent in fresh tempdir: {stdout}"
    );

    // dictionary.builtin_rules: positive int (we ship a non-empty default).
    assert!(
        v["dictionary"]["builtin_rules"].as_u64().unwrap_or(0) > 0,
        "dictionary.builtin_rules should be >0: {stdout}"
    );

    // No daemon started in this test → running:false. (The pidfile path
    // doesn't exist yet.)
    assert_eq!(
        v["daemon"]["running"].as_bool(),
        Some(false),
        "daemon should not be running in fresh tempdir: {stdout}"
    );
}

#[test]
fn config_show_works_without_index() {
    // Fresh workspace, no `.code-abyss/index.db` — config show must still
    // succeed and report `index_present:false`, counts:0. This is the
    // first-touch state new users hit.
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("--json")
        .arg("config")
        .arg("show")
        .output()
        .expect("spawn abyss config show");
    assert!(
        out.status.success(),
        "config show exit nonzero on bare workspace: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("invalid JSON on bare workspace");
    assert_eq!(v["index_present"].as_bool(), Some(false));
    assert_eq!(v["counts"]["files"].as_i64().unwrap_or(-1), 0);
    assert_eq!(v["schema_version"].as_i64().unwrap_or(-1), 0);
}
