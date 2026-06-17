//! V2 daemon: MCP-over-socket smoke tests.
//!
//! These pin three load-bearing contracts:
//! 1. `{"cmd":"mcp"}` switches the daemon socket into rmcp stdio mode and a
//!    standard JSON-RPC `initialize` round-trips successfully.
//! 2. `abyss mcp --via-daemon` errors with a clear "no daemon found"
//!    message when the socket is missing — the flag is opt-in, the
//!    fallback to standalone mode is by dropping the flag, not by silent
//!    auto-detect.
//! 3. Three concurrent MCP-mode connections all serve `tools/list` without
//!    SQLITE_BUSY, proving the read-only WAL multi-reader path works.
//!
//! Linux-only — same constraint as the V1 socket test.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use code_abyss::config::Config;
use code_abyss::indexer::IndexPipeline;
use code_abyss::storage::Repository;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
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

/// Seed a workspace + initial index so the daemon has a DB to serve.
fn seed_workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(
        ws.join("main.go"),
        "package main\n\nfunc Alpha() {}\nfunc Beta() { Alpha() }\n",
    )
    .unwrap();
    let config = Config::new(&ws);
    {
        let repo = Repository::open(&config.db_path, config.model.dimensions).unwrap();
        let pipeline = IndexPipeline::new(config.clone());
        pipeline.run_structural(&repo).unwrap();
    }
    (dir, ws)
}

fn spawn_daemon(ws: &Path) -> DaemonGuard {
    let child = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(ws)
        .arg("daemon")
        .arg("start")
        .arg("--foreground")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn abyss daemon");
    DaemonGuard(child)
}

/// Open a socket connection, send `{"cmd":"mcp"}`, then a JSON-RPC
/// `initialize`. Read the first response line and return it.
fn mcp_initialize_handshake(socket: &Path) -> String {
    let mut stream = UnixStream::connect(socket).expect("connect daemon.sock");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // Switch verb.
    writeln!(stream, r#"{{"cmd":"mcp"}}"#).unwrap();
    stream.flush().unwrap();

    // Standard MCP initialize.
    let initialize = r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#;
    writeln!(stream, "{initialize}").unwrap();
    stream.flush().unwrap();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read mcp response");
    line.trim().to_string()
}

#[test]
fn spawn_daemon_serves_mcp() {
    let (_tmp, ws) = seed_workspace();
    let _guard = spawn_daemon(&ws);

    let socket = ws.join(".code-abyss").join("daemon.sock");
    assert!(
        wait_for_path(&socket, Duration::from_secs(5)),
        "socket never appeared at {}",
        socket.display()
    );

    let resp = mcp_initialize_handshake(&socket);
    let v: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or_else(|_| panic!("non-JSON MCP response: {resp:?}"));
    assert_eq!(v["jsonrpc"], "2.0", "expected JSON-RPC envelope: {resp}");
    assert_eq!(v["id"], 1, "id should match initialize request: {resp}");
    // `result` must exist and carry serverInfo per the MCP spec.
    let server_info = &v["result"]["serverInfo"];
    assert!(
        server_info.is_object(),
        "missing result.serverInfo in: {resp}"
    );
}

#[test]
fn mcp_via_daemon_falls_back_on_no_socket() {
    // Fresh tempdir with no daemon — --via-daemon should error with a
    // specific message instead of silently spawning a standalone server.
    let dir = tempfile::tempdir().unwrap();
    let ws = std::fs::canonicalize(dir.path()).unwrap();

    let out = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("mcp")
        .arg("--via-daemon")
        .output()
        .expect("spawn abyss mcp --via-daemon");

    assert!(
        !out.status.success(),
        "expected failure when no daemon is running"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no daemon found"),
        "expected 'no daemon found' diagnostic, got stderr: {stderr}"
    );
    assert!(
        stderr.contains("daemon start") || stderr.contains("via-daemon"),
        "expected actionable hint about daemon start / --via-daemon, got: {stderr}"
    );
}

#[test]
fn multi_reader_smoke() {
    // Open three MCP-mode connections in parallel; each runs initialize +
    // tools/list and we assert all three get valid responses. Tests that
    // the read-only Repository handles don't contend at the SQLite layer.
    let (_tmp, ws) = seed_workspace();
    let _guard = spawn_daemon(&ws);

    let socket = ws.join(".code-abyss").join("daemon.sock");
    assert!(
        wait_for_path(&socket, Duration::from_secs(5)),
        "socket never appeared at {}",
        socket.display()
    );

    let mut handles = Vec::new();
    for client_id in 0..3 {
        let sock = socket.clone();
        handles.push(std::thread::spawn(move || -> String {
            let mut stream = UnixStream::connect(&sock).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();

            writeln!(stream, r#"{{"cmd":"mcp"}}"#).unwrap();
            stream.flush().unwrap();

            // initialize (id 1) then tools/list (id 2). We read both
            // responses so we can assert the channel kept working.
            let initialize = format!(
                r#"{{"jsonrpc":"2.0","method":"initialize","id":1,"params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"smoke-{client_id}","version":"0"}}}}}}"#,
            );
            writeln!(stream, "{initialize}").unwrap();
            stream.flush().unwrap();

            // The MCP spec requires `notifications/initialized` after the
            // initialize response. Send it before any subsequent request.
            let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

            let mut reader = BufReader::new(stream.try_clone().unwrap());

            // Read initialize response.
            let mut init_line = String::new();
            reader.read_line(&mut init_line).expect("read init resp");

            // Send `notifications/initialized` + `tools/list` once init came back.
            writeln!(stream, "{initialized}").unwrap();
            stream.flush().unwrap();
            writeln!(
                stream,
                r#"{{"jsonrpc":"2.0","method":"tools/list","id":2,"params":{{}}}}"#
            )
            .unwrap();
            stream.flush().unwrap();

            let mut list_line = String::new();
            reader.read_line(&mut list_line).expect("read tools resp");

            format!("init={} list={}", init_line.trim(), list_line.trim())
        }));
    }

    for (i, h) in handles.into_iter().enumerate() {
        let combined = h.join().expect("client thread panicked");
        // Both responses must be valid JSON envelopes with the expected ids.
        // Split on a unique sentinel — we control both halves.
        let init_part = combined
            .split(" list=")
            .next()
            .unwrap()
            .trim_start_matches("init=");
        let list_part = combined.split(" list=").nth(1).unwrap_or("");
        let init: serde_json::Value = serde_json::from_str(init_part)
            .unwrap_or_else(|_| panic!("client {i} init not JSON: {init_part:?}"));
        let list: serde_json::Value = serde_json::from_str(list_part)
            .unwrap_or_else(|_| panic!("client {i} tools/list not JSON: {list_part:?}"));
        assert_eq!(init["id"], 1, "client {i}: init id mismatch");
        assert_eq!(list["id"], 2, "client {i}: tools/list id mismatch");
        // tools array should be present and non-empty (the 7-tool surface).
        let tools = list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("client {i}: tools/list missing tools array: {list_part}"));
        assert!(
            !tools.is_empty(),
            "client {i}: tools/list returned empty array"
        );
    }
}
