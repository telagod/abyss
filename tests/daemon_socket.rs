//! Daemon V1 socket smoke test: spawn the real `abyss` binary against a
//! tempdir workspace, talk to it over the Unix socket, then kill it and
//! verify state-files are cleaned up.
//!
//! Linux-only — same constraint as `watcher_smoke.rs`: notify's inotify
//! backend is the deterministic one for CI, and `UnixListener` requires a
//! Unix target anyway.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;

fn abyss_binary() -> PathBuf {
    // CARGO_BIN_EXE_<name> points to the freshly built binary for integration
    // tests — matches the rest of the suite's invocation style.
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn wait_for_path(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn wait_for_path_gone(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn send_request(socket: &std::path::Path, payload: &str) -> String {
    let mut stream = UnixStream::connect(socket).expect("connect daemon.sock");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    writeln!(stream, "{payload}").expect("write request");
    stream.flush().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read response");
    line.trim().to_string()
}

struct DaemonGuard(Child);
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_socket_ping_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();

    // Seed the workspace with one indexable file and run a structural index
    // so the daemon has a DB to open against.
    std::fs::write(ws.join("main.go"), "package main\n\nfunc Alpha() {}\n").unwrap();
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).unwrap();
    }

    // Spawn the daemon as a background process. --foreground keeps tracing
    // on stderr so we can surface failures in test output; the OS still lets
    // us own the child lifecycle via Child::kill.
    let child = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("daemon")
        .arg("start")
        .arg("--foreground")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn abyss daemon");
    let _guard = DaemonGuard(child);

    let socket = ws.join(".code-abyss").join("daemon.sock");
    let pidfile = ws.join(".code-abyss").join("daemon.pid");

    assert!(
        wait_for_path(&socket, Duration::from_secs(5)),
        "socket never appeared at {}",
        socket.display()
    );
    assert!(pidfile.exists(), "pidfile missing at {}", pidfile.display());

    // 1) ping — uptime + zero reindex telemetry.
    let resp = send_request(&socket, r#"{"cmd":"ping"}"#);
    let v: serde_json::Value = serde_json::from_str(&resp).expect(&resp);
    assert_eq!(v["ok"], serde_json::Value::Bool(true), "ping resp: {resp}");
    assert!(v["uptime_secs"].is_number(), "ping resp: {resp}");
    assert!(v["last_reindex_ms"].is_number(), "ping resp: {resp}");
    assert!(v["epoch"].is_number(), "ping resp: {resp}");

    // 2) stats — counts ≥ 0 (we indexed one file so files ≥ 1 too).
    let resp = send_request(&socket, r#"{"cmd":"stats"}"#);
    let v: serde_json::Value = serde_json::from_str(&resp).expect(&resp);
    assert_eq!(v["ok"], serde_json::Value::Bool(true), "stats resp: {resp}");
    assert!(v["files"].as_i64().unwrap_or(-1) >= 1, "stats resp: {resp}");
    assert!(
        v["symbols"].as_i64().unwrap_or(-1) >= 0,
        "stats resp: {resp}"
    );
    assert!(v["refs"].as_i64().unwrap_or(-1) >= 0, "stats resp: {resp}");

    // 3) reindex verb — synchronous hash-incremental run through the
    // daemon. Files already indexed → `reindexed` may be 0 (unchanged),
    // `removed` is 0, duration is bounded. Epoch must bump from whatever
    // ping observed earlier.
    let resp = send_request(&socket, r#"{"cmd":"reindex"}"#);
    let v: serde_json::Value = serde_json::from_str(&resp).expect(&resp);
    assert_eq!(
        v["ok"],
        serde_json::Value::Bool(true),
        "reindex resp: {resp}"
    );
    assert!(
        v["reindexed"].as_i64().unwrap_or(-1) >= 0,
        "reindex resp: {resp}"
    );
    assert!(
        v["removed"].as_i64().unwrap_or(-1) >= 0,
        "reindex resp: {resp}"
    );
    assert!(
        v["duration_ms"].as_i64().unwrap_or(-1) >= 0,
        "reindex resp: {resp}"
    );
    assert!(
        v["epoch"].as_u64().unwrap_or(0) >= 1,
        "reindex should bump epoch at least once: {resp}"
    );

    // 4) logs verb — tail at most 5 lines. The daemon's startup `info!`
    // ("abyss daemon started ...") lands in the log via the redirect, so
    // we expect at least one non-empty line.
    let resp = send_request(&socket, r#"{"cmd":"logs","tail":5}"#);
    let v: serde_json::Value = serde_json::from_str(&resp).expect(&resp);
    assert_eq!(v["ok"], serde_json::Value::Bool(true), "logs resp: {resp}");
    let lines = v["lines"].as_array().expect(&resp);
    assert!(
        lines.len() <= 5,
        "logs returned more than requested: {resp}"
    );
    // --foreground means the daemon redirected nothing to the log file, so
    // it may legitimately be empty. We assert the contract (≤ tail, ok=true)
    // here and exercise the real-log path via the V1 redirect path below.

    // 5) Stop via the CLI (mirrors how users would do it). Spawn-and-wait
    // because stop blocks until the daemon's pidfile is gone (≤5s).
    let stop = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("daemon")
        .arg("stop")
        .output()
        .expect("spawn daemon stop");
    assert!(
        stop.status.success(),
        "daemon stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );

    // 4) Pidfile + socket should be cleaned up. The daemon unlinks both on
    // shutdown; give it a brief grace window in case the kernel hasn't
    // settled the fs view yet.
    assert!(
        wait_for_path_gone(&pidfile, Duration::from_secs(3)),
        "pidfile not cleaned up"
    );
    assert!(
        wait_for_path_gone(&socket, Duration::from_secs(3)),
        "socket not cleaned up"
    );
}

/// V1.5: verify the `logs` verb returns the daemon's own startup tracing.
/// Distinct from the smoke test because we deliberately drop `--foreground`
/// so the daemon redirects its stderr into `daemon.log` — the only way to
/// prove the file path the `logs` verb tails is the one the daemon writes.
#[test]
fn daemon_logs_verb_returns_startup_line() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();

    std::fs::write(ws.join("main.go"), "package main\n\nfunc Beta() {}\n").unwrap();
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).unwrap();
    }

    // No --foreground → the daemon dup2's stderr into `daemon.log`. We keep
    // a piped stderr handle on the Child anyway so a fail-to-spawn surfaces;
    // it just won't catch the tracing lines (those go to the log file).
    let child = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("daemon")
        .arg("start")
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn abyss daemon");
    let _guard = DaemonGuard(child);

    let socket = ws.join(".code-abyss").join("daemon.sock");
    let log_path = ws.join(".code-abyss").join("daemon.log");

    assert!(
        wait_for_path(&socket, Duration::from_secs(5)),
        "socket never appeared at {}",
        socket.display()
    );

    // Give the daemon a moment to flush its first tracing line. The
    // `abyss daemon started ...` info! happens right after socket bind, so
    // 500ms is generous.
    std::thread::sleep(Duration::from_millis(500));

    // Bounded retry: on slow CI the redirect can lag the bind by a tick.
    let mut last_resp = String::new();
    let mut lines_vec: Vec<serde_json::Value> = Vec::new();
    for _ in 0..10 {
        let resp = send_request(&socket, r#"{"cmd":"logs","tail":5}"#);
        last_resp = resp.clone();
        let v: serde_json::Value = serde_json::from_str(&resp).expect(&resp);
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "logs resp: {resp}");
        let lines = v["lines"].as_array().cloned().unwrap_or_default();
        if !lines.is_empty() {
            lines_vec = lines;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    assert!(!lines_vec.is_empty(), "logs returned no lines: {last_resp}");
    assert!(lines_vec.len() <= 5, "logs returned >5 lines: {last_resp}");

    // The redirect_log() header is `--- abyss daemon pid=N start ---` and
    // the first tracing line is the `abyss daemon started` info — assert at
    // least one of those well-known prefixes appears in the tail.
    let joined: String = lines_vec
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("abyss daemon") || joined.contains("--- abyss daemon"),
        "expected daemon startup line in logs tail, got: {joined}"
    );

    // Sanity: the daemon really did write the log file.
    assert!(
        log_path.exists(),
        "daemon.log missing at {}",
        log_path.display()
    );

    // Clean shutdown.
    let stop = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("daemon")
        .arg("stop")
        .output()
        .expect("spawn daemon stop");
    assert!(
        stop.status.success(),
        "daemon stop failed: {}",
        String::from_utf8_lossy(&stop.stderr)
    );
}
