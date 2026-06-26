//! Unix-socket request/response loop. Newline-delimited JSON.
//!
//! Protocol (V2):
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
//!
//! → {"cmd": "subscribe"}
//! ← {"ok": true, "subscribed": true}          (ack, then…)
//! ← {"event": "reindexed", "files": N, "epoch": E, "ts": T}
//! ← {"event": "reindexed", "files": N, "epoch": E, "ts": T}
//! ← … one line per reindex burst, until the client disconnects.
//!
//! → {"cmd": "mcp"}
//! ← (no JSON envelope — the connection switches into MCP stdio mode and
//!     the daemon serves the standard 7-tool surface over the same fd via
//!     newline-delimited JSON-RPC. Backed by a per-connection read-only
//!     SQLite handle so concurrent MCP clients don't contend with the
//!     watcher's writer.)
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
//! The `mcp` verb (V2) takes ownership of the connection for its lifetime
//! and runs an rmcp service over the same socket. Each MCP-mode connection
//! gets its own [`Repository::open_read_only`] handle so we exercise WAL's
//! N-readers-+-1-writer property cleanly.

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

    // Restrict socket to owner-only (0o700) so other local users cannot
    // connect and issue reindex / MCP / subscribe commands.
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(socket_path, perms)
            .with_context(|| format!("chmod 0700 {}", socket_path.display()))?;
    }

    listener
        .set_nonblocking(true)
        .context("set_nonblocking on UnixListener")?;

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // The listener is non-blocking (so accept() can poll the
                // shutdown flag) and Linux inherits that flag onto accepted
                // sockets. Flip the new stream back to blocking before
                // handing it to the worker thread: every code path below
                // (request/response, MCP-mode tokio conversion) expects to
                // configure the mode explicitly.
                if let Err(e) = stream.set_nonblocking(false) {
                    tracing::warn!("daemon socket: clear nonblocking failed: {e}");
                    continue;
                }
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

    // Deliberately *not* a BufReader. The `mcp` verb hands the raw fd off
    // to rmcp's async reader, so any bytes the BufReader already pulled
    // off the kernel buffer would be lost. read_line_unbuffered reads one
    // byte at a time — slow on paper but the request rate here is one
    // line per operator action, so the cost is invisible.
    let mut read_clone = stream.try_clone()?;
    let mut writer = stream;

    loop {
        let line = match read_line_unbuffered(&mut read_clone)? {
            Some(l) => l,
            None => break, // peer closed cleanly
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        // Peek the verb before dispatch so the `mcp` and `subscribe`
        // verbs can grab the whole connection instead of returning a
        // single response. Every other verb stays in the
        // request/response shape.
        let parsed: serde_json::Result<Request> = serde_json::from_str(&line);
        match &parsed {
            Ok(req) if req.cmd == "mcp" => {
                // Drop the read clone before we take the stream back.
                // Both halves share the same fd; releasing the clone keeps
                // the rmcp transport's view of the socket unambiguous.
                drop(read_clone);
                // Clear the 5s I/O timeouts inherited from the
                // request/response path — MCP sessions are long-lived
                // and rmcp drives its own backpressure.
                writer.set_read_timeout(None).ok();
                writer.set_write_timeout(None).ok();
                return serve_mcp_connection(writer, state);
            }
            Ok(req) if req.cmd == "subscribe" => {
                drop(read_clone);
                // Subscribe sessions are long-lived; relax the 5s
                // write timeout so a slow consumer doesn't reset us
                // mid-event, but keep it bounded so a totally-stuck
                // peer eventually closes (60s is generous yet still
                // surfaces wedged clients).
                writer.set_read_timeout(None).ok();
                writer.set_write_timeout(Some(Duration::from_secs(60))).ok();
                return serve_subscribe_connection(writer, state);
            }
            _ => {}
        }

        let response = match parsed {
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

/// Read a single `\n`-terminated line from a raw stream without buffering
/// any bytes past the newline. Returns `Ok(None)` on a clean EOF before the
/// first byte. Used by [`handle_conn`] so the `mcp` switch verb can hand
/// the rest of the socket off to rmcp untouched.
fn read_line_unbuffered(stream: &mut UnixStream) -> Result<Option<String>> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(64);
    let mut one = [0u8; 1];
    loop {
        let n = stream.read(&mut one)?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                // Treat orphan trailing bytes as a terminated line. Caller
                // will fail-parse if it's not valid JSON.
                Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
            };
        }
        if one[0] == b'\n' {
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        buf.push(one[0]);
        // Defensive cap — a malformed client shouldn't be able to spool an
        // unbounded buffer here. 64KiB is far past any sane verb payload.
        if buf.len() > 65_536 {
            anyhow::bail!("daemon socket: line too long (>64KiB)");
        }
    }
}

/// Hand the socket off to an rmcp service. Each connection gets a private
/// read-only SQLite handle (multi-reader path on WAL) plus a fresh tokio
/// current-thread runtime — we deliberately don't share a runtime across
/// connections so a slow MCP client can't block stats/ping/reindex traffic
/// running on other socket-handler threads.
fn serve_mcp_connection(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    // Tokio requires non-blocking std sockets before `from_std`.
    stream
        .set_nonblocking(true)
        .context("set_nonblocking on MCP socket")?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .context("build per-connection tokio runtime")?;

    rt.block_on(async move {
        let async_stream =
            tokio::net::UnixStream::from_std(stream).context("wrap std UnixStream into tokio")?;

        // Per-connection read-only repo. Construction errors are surfaced
        // to the client as a JSON-RPC-shaped log line on the daemon side —
        // returning the error here just drops the connection, which is
        // also what rmcp does on its own transport errors.
        let repo = crate::storage::Repository::open_read_only(&state.db_path, state.dimensions)
            .context("open read-only repo for MCP connection")?;

        // Embedder is daemon-local (writer side); the MCP read-only handle
        // intentionally doesn't try to share it. Slim builds never had one
        // anyway, semantic builds get fulltext-only over the socket path
        // (search still returns useful results; the `precision_mode` field
        // tells the agent which mode actually ran).
        let embedder: Option<crate::embedding::Embedder> = None;

        let pipeline = crate::indexer::IndexPipeline::new(state.config.clone());

        let server = crate::mcp::McpServer {
            repo: std::sync::Arc::new(std::sync::Mutex::new(repo)),
            embedder: std::sync::Arc::new(embedder),
            pipeline: std::sync::Arc::new(pipeline),
            config: state.config.clone(),
        };

        let (read_half, write_half) = tokio::io::split(async_stream);

        use rmcp::ServiceExt;
        let service = server
            .serve((read_half, write_half))
            .await
            .context("rmcp serve over Unix socket")?;
        // Wait until the client disconnects. Errors here are connection-
        // level (peer reset, EOF) and not interesting to surface.
        let _ = service.waiting().await;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

/// Subscribe-mode handler. Registers an mpsc receiver against the
/// daemon's subscriber list, writes an ack line so the client knows the
/// subscription went through, then blocks reading the receiver and
/// forwarding each [`crate::daemon::state::ReindexEvent`] as a single
/// newline-delimited JSON line.
///
/// Returns when either the watcher's sender side is dropped (daemon
/// shutting down — `RecvError`) or a write fails (peer disconnected).
/// Both are normal termination paths; we don't surface them as errors.
fn serve_subscribe_connection(mut stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let rx = state.register_subscriber();

    // ACK so the client can synchronously distinguish "registered" from
    // "the daemon was already going away". `subscribed: true` is the
    // version-tagged flag; if we ever add `subscribe_v2` semantics, an
    // older client can refuse based on the absence of a new field.
    let ack = serde_json::json!({"ok": true, "subscribed": true});
    writeln!(stream, "{ack}")?;
    stream.flush()?;

    loop {
        // Block until either the watcher pushes an event or every
        // sender drops (daemon shutdown). 60s recv_timeout lets us
        // periodically write a no-op keepalive when nothing's
        // happening — without it, a long-idle subscriber holds a
        // socket fd with no liveness probe and the kernel can take
        // hours to notice a dead peer.
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(event) => {
                let line = match serde_json::to_string(&event) {
                    Ok(l) => l,
                    Err(e) => {
                        // Serialisation should be infallible for our
                        // event shape; if it ever fails, log and skip
                        // rather than tearing down the connection.
                        tracing::warn!("subscribe: encode event failed: {e}");
                        continue;
                    }
                };
                if writeln!(stream, "{line}").is_err() {
                    // Peer closed — normal shutdown path. retain() on
                    // the next broadcast will prune our sender.
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Keepalive — a one-byte newline is the lightest probe
                // that still exercises the socket write path. Clients
                // ignoring whitespace between JSON lines is the
                // newline-delimited-JSON convention.
                if writeln!(stream).is_err() {
                    break;
                }
                if stream.flush().is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Watcher / daemon is shutting down — all senders dropped.
                break;
            }
        }
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
