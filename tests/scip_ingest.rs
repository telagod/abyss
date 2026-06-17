//! `abyss ingest scip` prototype smoke tests (v0.5.15).
//!
//! The full DB-write path is not wired yet — the prototype only parses
//! and summarises. These tests pin the CLI surface so a future patch
//! that lights up the L-1 tier can't silently break the script-facing
//! flags / counts.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

const MOCK_INDEX: &str = r#"{
    "metadata": {
        "version": "0.6.0",
        "project_root": "file:///tmp/proj",
        "tool_info": { "name": "scip-go", "version": "0.1.0" }
    },
    "documents": [
        {
            "relative_path": "src/a.go",
            "language": "go",
            "occurrences": [
                {
                    "range": [10, 4, 14],
                    "symbol": "scip-go pkg/a/Foo.",
                    "symbol_roles": 1
                },
                {
                    "range": [20, 8, 18],
                    "symbol": "scip-go pkg/b/Bar#",
                    "symbol_roles": 0
                }
            ]
        }
    ]
}"#;

/// `abyss ingest scip <file.json> --dry-run --print-summary` exits 0
/// and prints the document / occurrence counts to stderr. The
/// JSON-via-`--json` shape is the contract the future MCP
/// `ingest_scip` tool will return verbatim.
#[test]
fn ingest_scip_dry_run_prints_summary() {
    let dir = tempfile::tempdir().unwrap();
    let scip_json = dir.path().join("index.json");
    std::fs::write(&scip_json, MOCK_INDEX).unwrap();

    let out = Command::new(abyss_binary())
        .arg("ingest")
        .arg("scip")
        .arg(&scip_json)
        .arg("--dry-run")
        .arg("--print-summary")
        .output()
        .expect("spawn abyss ingest scip");
    assert!(
        out.status.success(),
        "ingest scip failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("1 documents"),
        "expected document count in stderr: {stderr}"
    );
    assert!(
        stderr.contains("2 occurrences"),
        "expected occurrence count in stderr: {stderr}"
    );
    assert!(
        stderr.contains("prototype"),
        "summary should mention prototype gate: {stderr}"
    );
}

/// `--json` round-trips the summary verbatim so the MCP layer can pin
/// the same shape later.
#[test]
fn ingest_scip_dry_run_json_returns_summary() {
    let dir = tempfile::tempdir().unwrap();
    let scip_json = dir.path().join("index.json");
    std::fs::write(&scip_json, MOCK_INDEX).unwrap();

    let out = Command::new(abyss_binary())
        .arg("--json")
        .arg("ingest")
        .arg("scip")
        .arg(&scip_json)
        .arg("--dry-run")
        .output()
        .expect("spawn abyss ingest scip --json");
    assert!(
        out.status.success(),
        "ingest scip --json failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("parse json summary: {stdout}"));
    assert_eq!(summary["documents"], 1);
    assert_eq!(summary["occurrences"], 2);
    assert_eq!(summary["ref_candidates"], 1);
    assert_eq!(summary["definitions"], 1);
    assert!(
        summary["languages"]
            .as_array()
            .unwrap()
            .contains(&"go".into()),
        "languages should include go: {summary}"
    );
}

/// Binary `.scip` extension must surface an actionable error pointing
/// at the conversion command so operators aren't left guessing why
/// nothing happened.
#[test]
fn ingest_scip_binary_extension_errors_with_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let scip_bin = dir.path().join("index.scip");
    std::fs::write(&scip_bin, b"\x00not actually a real scip blob\x00").unwrap();

    let out = Command::new(abyss_binary())
        .arg("ingest")
        .arg("scip")
        .arg(&scip_bin)
        .arg("--dry-run")
        .output()
        .expect("spawn abyss ingest scip binary");
    assert!(
        !out.status.success(),
        "binary .scip should fail in v0.5.15: stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("scip print --json"),
        "should suggest the conversion command: {stderr}"
    );
}

/// Dropping `--dry-run` must error out — the full ingest path is not
/// wired yet, and a silent no-op would mislead operators into thinking
/// the index was updated.
#[test]
fn ingest_scip_without_dry_run_errors_out() {
    let dir = tempfile::tempdir().unwrap();
    let scip_json = dir.path().join("index.json");
    std::fs::write(&scip_json, MOCK_INDEX).unwrap();

    let out = Command::new(abyss_binary())
        .arg("ingest")
        .arg("scip")
        .arg(&scip_json)
        .output()
        .expect("spawn abyss ingest scip without dry-run");
    assert!(
        !out.status.success(),
        "non-dry-run should fail in v0.5.15: stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not implemented"),
        "should say feature is unimplemented: {stderr}"
    );
}
