//! Unix-socket request/response loop. Newline-delimited JSON.
//!
//! Protocol (V1.5):
//! ```text
//! → {"cmd": "ping"}
//! ← {"ok": true, "uptime_secs": N, "last_reindex_ms": M, "epoch": E}
//!
//! → {"cmd": "stats"}
//! ← {"ok": true, "files": N, "symbols": M, "refs": K, "chunks": C, "epoch": E}
//!
//! → {"cmd": "reindex"}
//! ← {"ok": true, "reindexed": N, "removed": M, "duration_ms": D, "epoch": E}
//!
//! → {"cmd": "logs", "tail": N}
//! ← {"ok": true, "lines": ["...", "..."]}
//! ```
//! Anything else: `{"ok": false, "error": "..."}`.
//!
//! The `reindex` verb runs a synchronous hash-incremental
//! [`crate::indexer::IndexPipeline::run_structural`] on a worker thread —
//! same path the watcher exercises on debounced FS events, just operator-
//! triggered. Serialized against itself via `state.reindex_lock`; two
//! concurrent reindex requests will see one succeed and one get
//! `{"ok": false, "error": "index lock contention"}`.
//!
//! Deliberately still minimal — the full MCP-over-socket surface is V2.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::daemon::DaemonState;
use crate::indexer::IndexPipeline;
use crate::storage::Repository;

#[derive(Debug, Deserialize)]
struct Request {
    cmd: String,
    /// `logs` verb: how many trailing lines to return. Defaults to 50 if
    /// omitted, matching the CLI default.
    #[serde(default)]
    tail: Option<usize>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ReindexResponse {
    pub ok: bool,
    pub reindexed: u64,
    pub removed: u64,
    pub duration_ms: u64,
    pub epoch: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogsResponse {
    pub ok: bool,
    pub lines: Vec<String>,
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
        "reindex" => match reindex_payload(state) {
            Ok(s) => s,
            Err(e) => serde_json::to_string(&ErrorResponse {
                ok: false,
                error: format!("reindex: {e}"),
            })
            .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"encode failure\"}".into()),
        },
        "logs" => {
            // Default to 50 trailing lines if the client omitted `tail`;
            // matches the `abyss daemon logs` CLI default.
            let tail = req.tail.unwrap_or(50);
            match logs_payload(state, tail) {
                Ok(s) => s,
                Err(e) => serde_json::to_string(&ErrorResponse {
                    ok: false,
                    error: format!("logs: {e}"),
                })
                .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"encode failure\"}".into()),
            }
        }
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

fn reindex_payload(state: &DaemonState) -> Result<String> {
    // `try_lock` makes the contention case explicit: two operator-triggered
    // reindexes shouldn't queue silently — the second client deserves to
    // know we're busy so it can back off or retry.
    let guard = match state.reindex_lock.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return Ok(serde_json::to_string(&ErrorResponse {
                ok: false,
                error: "index lock contention".into(),
            })?);
        }
    };

    let repo = Repository::open(&state.db_path, state.dimensions)?;
    let pipeline = IndexPipeline::new(state.config.clone());
    let stats = pipeline.run_structural(&repo)?;

    // Bump the daemon's reindex telemetry so `ping` sees the operator-
    // triggered run too — clients polling `epoch` need this to notice.
    state.record_reindex(stats.duration_ms);
    drop(guard);

    let payload = ReindexResponse {
        ok: true,
        reindexed: stats.indexed,
        removed: stats.deleted,
        duration_ms: stats.duration_ms,
        epoch: state.epoch.load(Ordering::Relaxed),
    };
    Ok(serde_json::to_string(&payload)?)
}

fn logs_payload(state: &DaemonState, tail: usize) -> Result<String> {
    let lines = tail_log(&state.log_path, tail)?;
    let payload = LogsResponse { ok: true, lines };
    Ok(serde_json::to_string(&payload)?)
}

/// Streaming tail: read the file once and keep only the last `n` lines in a
/// bounded `VecDeque`. Memory stays O(n), not O(file size) — important
/// because `daemon.log` is append-only across restarts and can grow.
/// Missing log file is treated as "no lines" rather than an error so the
/// CLI works cleanly the first time you ask before any logs have been
/// flushed.
fn tail_log(log_path: &Path, n: usize) -> Result<Vec<String>> {
    if !log_path.exists() || n == 0 {
        return Ok(Vec::new());
    }
    let file =
        std::fs::File::open(log_path).with_context(|| format!("open {}", log_path.display()))?;
    let reader = BufReader::new(file);
    let mut buf: VecDeque<String> = VecDeque::with_capacity(n);
    for line in reader.lines() {
        let line = line.unwrap_or_default();
        if buf.len() == n {
            buf.pop_front();
        }
        buf.push_back(line);
    }
    Ok(buf.into_iter().collect())
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

/// Client helper: fetch the last `tail` lines of the daemon log. The CLI
/// `abyss daemon logs --tail N` is a thin wrapper around this. Reindex
/// might take seconds, so the read timeout is generous; logs reads are
/// fast (bounded by file size on the daemon side).
pub fn logs(socket_path: &Path, tail: usize) -> Result<LogsResponse> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect {}", socket_path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let req = serde_json::json!({"cmd": "logs", "tail": tail});
    writeln!(stream, "{req}")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp: LogsResponse = serde_json::from_str(line.trim())
        .with_context(|| format!("decode logs response: {line:?}"))?;
    Ok(resp)
}
