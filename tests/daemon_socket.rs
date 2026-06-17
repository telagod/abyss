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

    // 3) Stop via the CLI (mirrors how users would do it). Spawn-and-wait
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
