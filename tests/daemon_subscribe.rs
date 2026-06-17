//! Daemon v0.5.16 push-notification verb (`subscribe`).
//!
//! Spins up the real daemon, opens a Unix-socket connection, sends
//! `{"cmd":"subscribe"}`, then mutates a watched file. The watcher's
//! `on_reindex` callback fires `broadcast_reindex` which fans the event
//! out to every active subscriber. We assert:
//!   1. The connection receives an immediate `subscribed: true` ack so
//!      the client knows the registration landed.
//!   2. A `{"event":"reindexed", …}` line arrives within 1.5 s of the
//!      file save (debounce 150 ms + reindex slack).
//!   3. The event payload carries `files >= 1`, a non-zero `epoch`, and
//!      a sensible `ts`.
//!
//! Linux-only — same constraint as the rest of the daemon tests
//! (inotify is the only deterministic backend; `UnixListener` is POSIX).

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

struct DaemonGuard(Child);
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Read one newline-delimited JSON object from the stream, skipping any
/// keepalive blank lines the subscribe handler emits during idle
/// timeouts. Returns the raw line so callers can decode + assert.
fn read_next_event(reader: &mut BufReader<UnixStream>, deadline: Instant) -> Option<String> {
    while Instant::now() < deadline {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok()?;
        if n == 0 {
            return None; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // keepalive — keep waiting
        }
        return Some(trimmed.to_string());
    }
    None
}

#[test]
fn subscribe_receives_reindex_event_on_file_save() {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();

    // Seed one Go file so the daemon has something to watch and the
    // index opens cleanly.
    std::fs::write(ws.join("main.go"), "package main\n\nfunc Alpha() {}\n").unwrap();
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).unwrap();
    }

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
    assert!(
        wait_for_path(&socket, Duration::from_secs(5)),
        "socket never appeared at {}",
        socket.display()
    );

    // Open the subscribe connection BEFORE mutating the file so the
    // sender is registered when the reindex burst fires.
    let stream = UnixStream::connect(&socket).expect("connect daemon.sock");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // Send the subscribe verb and read the ack.
    {
        let mut writer = stream.try_clone().expect("clone stream for write");
        writeln!(writer, r#"{{"cmd":"subscribe"}}"#).expect("write subscribe");
        writer.flush().unwrap();
    }

    let mut reader = BufReader::new(stream);
    let mut ack_line = String::new();
    reader.read_line(&mut ack_line).expect("read ack");
    let ack: serde_json::Value =
        serde_json::from_str(ack_line.trim()).unwrap_or_else(|_| panic!("ack json: {ack_line}"));
    assert_eq!(
        ack["ok"],
        serde_json::Value::Bool(true),
        "subscribe ack: {ack_line}"
    );
    assert_eq!(
        ack["subscribed"],
        serde_json::Value::Bool(true),
        "subscribe ack: {ack_line}"
    );

    // Give the watcher a beat to install its inotify subscription —
    // mirrors watcher_smoke.rs's 250 ms hand-off slack.
    std::thread::sleep(Duration::from_millis(300));

    // Mutate the file. The watcher debounce is 150 ms; reindex should
    // fire and the event should land within 1.5 s comfortably.
    std::fs::write(
        ws.join("main.go"),
        "package main\n\nfunc Alpha() {}\nfunc Beta() {}\n",
    )
    .expect("save mutation");

    // Read the next non-keepalive line as the reindex event.
    let deadline = Instant::now() + Duration::from_secs(3);
    let raw = read_next_event(&mut reader, deadline)
        .expect("no reindex event received within 3s of save");
    let event: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| panic!("event json: {raw}"));
    assert_eq!(
        event["event"],
        serde_json::Value::String("reindexed".to_string()),
        "expected event=reindexed, got: {raw}",
    );
    assert!(
        event["files"].as_u64().unwrap_or(0) >= 1,
        "files should be >= 1, got: {raw}",
    );
    assert!(
        event["epoch"].as_u64().unwrap_or(0) >= 1,
        "epoch should be bumped, got: {raw}",
    );
    assert!(
        event["ts"].as_u64().unwrap_or(0) > 0,
        "ts should be a real unix timestamp, got: {raw}",
    );

    // Clean shutdown via the CLI.
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
