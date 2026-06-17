//! Unix-socket request/response loop. Newline-delimited JSON, two verbs.
//!
//! Protocol (V1):
//! ```text
//! → {"cmd": "ping"}
//! ← {"ok": true, "uptime_secs": N, "last_reindex_ms": M, "epoch": E}
//!
//! → {"cmd": "stats"}
//! ← {"ok": true, "files": N, "symbols": M, "refs": K, "chunks": C, "epoch": E}
//! ```
//! Anything else: `{"ok": false, "error": "..."}`.
//!
//! Deliberately minimal — the full MCP-over-socket surface is V2.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::daemon::DaemonState;
use crate::storage::Repository;

#[derive(Debug, Deserialize)]
struct Request {
    cmd: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PingResponse {
    pub ok: bool,
    pub uptime_secs: u64,
    pub last_reindex_ms: u64,
    pub epoch: u64,
}

#[derive(Debug, Serialize)]
struct StatsResponse {
    ok: bool,
    files: i64,
    chunks: i64,
    symbols: i64,
    refs: i64,
    epoch: u64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

/// Bind the socket and serve connections until `stop` flips to `true`. The
/// socket file is removed on a clean exit; partial state is best-effort.
pub fn serve(socket_path: &Path, state: Arc<DaemonState>, stop: Arc<AtomicBool>) -> Result<()> {
    // Stale socket from a previous crashed run? Remove it. The pidfile lock
    // (acquired before serve()) already proved we're the only daemon.
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    listener
        .set_nonblocking(true)
        .context("set_nonblocking on UnixListener")?;

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let st = state.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn(stream, st) {
                        tracing::debug!("daemon socket conn error: {e}");
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::warn!("daemon accept error: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

fn handle_conn(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(line) {
            Ok(req) => dispatch(&req, &state),
            Err(e) => serde_json::to_string(&ErrorResponse {
                ok: false,
                error: format!("malformed request: {e}"),
            })?,
        };

        writeln!(writer, "{response}")?;
        writer.flush()?;
    }
    Ok(())
}

fn dispatch(req: &Request, state: &DaemonState) -> String {
    match req.cmd.as_str() {
        "ping" => serde_json::to_string(&PingResponse {
            ok: true,
            uptime_secs: state.uptime_secs(),
            last_reindex_ms: state.last_reindex_ms.load(Ordering::Relaxed),
            epoch: state.epoch.load(Ordering::Relaxed),
        })
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"encode: {e}\"}}")),
        "stats" => match stats_payload(state) {
            Ok(s) => s,
            Err(e) => serde_json::to_string(&ErrorResponse {
                ok: false,
                error: format!("stats: {e}"),
            })
            .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"encode failure\"}".into()),
        },
        other => serde_json::to_string(&ErrorResponse {
            ok: false,
            error: format!("unknown cmd: {other}"),
        })
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"encode failure\"}".into()),
    }
}

fn stats_payload(state: &DaemonState) -> Result<String> {
    // Open a fresh read-only handle per request. SQLite + WAL is comfortable
    // with concurrent readers, and the cost is one open syscall (~100µs).
    let repo = Repository::open(&state.db_path, state.dimensions)?;
    let payload = StatsResponse {
        ok: true,
        files: repo.file_count()?,
        chunks: repo.chunk_count()?,
        symbols: repo.symbol_count()?,
        refs: repo.ref_count()?,
        epoch: state.epoch.load(Ordering::Relaxed),
    };
    Ok(serde_json::to_string(&payload)?)
}

/// Client helper: send `{"cmd":"ping"}` and parse the response. Used by
/// `abyss daemon status` to fetch telemetry without re-implementing the
/// wire format in two places.
pub fn ping(socket_path: &Path) -> Result<PingResponse> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect {}", socket_path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{{\"cmd\":\"ping\"}}")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp: PingResponse = serde_json::from_str(line.trim())
        .with_context(|| format!("decode ping response: {line:?}"))?;
    Ok(resp)
}
