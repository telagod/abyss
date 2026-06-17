//! Background daemon (V1 + V1.5) — single-instance pidfile lock + Unix-socket
//! request/response loop in front of the existing [`FileWatcher`].
//!
//! V1 scope:
//! - `abyss daemon start [--foreground]` — claim pidfile, bind socket, watch.
//! - `abyss daemon stop` — SIGTERM the recorded pid, wait up to 5s for cleanup.
//! - `abyss daemon status` — print pid + uptime + last reindex + socket path.
//! - Protocol verbs: `ping`, `stats`.
//!
//! V1.5 additions:
//! - `abyss daemon start --detach` — proper double-fork + setsid so the
//!   daemon survives the shell that launched it without `&`. Stdin is
//!   closed, stdout/stderr land in `.code-abyss/daemon.log`.
//! - `abyss daemon logs [--tail N]` — tail the daemon log. Goes through the
//!   socket when the daemon is live; falls back to a direct file read when
//!   it's not.
//! - Socket verbs: `reindex` (synchronous hash-incremental run on a worker
//!   thread, serialized via a try_lock'd mutex so two operator requests
//!   surface a structured "lock contention" error instead of fighting at
//!   the SQLite layer), `logs` (returns the trailing N lines).
//!
//! Unix-only — the watcher's inotify backend is already POSIX-shaped, and
//! `UnixListener` is unavailable on Windows. The Windows path lives in
//! [`mod@windows_stub`] and prints an instructive bail.

#[cfg(unix)]
pub mod pidfile;
#[cfg(unix)]
pub mod socket;
#[cfg(unix)]
pub mod state;

#[cfg(unix)]
pub use pidfile::PidFile;
#[cfg(unix)]
pub use state::DaemonState;

use anyhow::Result;
use std::path::PathBuf;

use crate::config::Config;

/// Subcommand routing — kept here so `main.rs` stays a thin clap dispatcher.
pub enum DaemonAction {
    Start { foreground: bool, detach: bool },
    Stop,
    Status,
    Logs { tail: usize, follow: bool },
}

/// Standard paths under `<workspace>/.code-abyss/` — shared by every daemon
/// helper so producer/consumer agree on a single location per workspace.
pub struct DaemonPaths {
    pub dir: PathBuf,
    pub pid: PathBuf,
    pub socket: PathBuf,
    pub log: PathBuf,
}

impl DaemonPaths {
    pub fn from_config(config: &Config) -> Self {
        let dir = config.workspace.join(".code-abyss");
        Self {
            pid: dir.join("daemon.pid"),
            socket: dir.join("daemon.sock"),
            log: dir.join("daemon.log"),
            dir,
        }
    }
}

#[cfg(unix)]
pub fn run(config: Config, action: DaemonAction) -> Result<()> {
    match action {
        DaemonAction::Start { foreground, detach } => unix_impl::start(config, foreground, detach),
        DaemonAction::Stop => unix_impl::stop(config),
        DaemonAction::Status => unix_impl::status(config),
        DaemonAction::Logs { tail, follow } => unix_impl::logs(config, tail, follow),
    }
}

#[cfg(not(unix))]
pub fn run(_config: Config, _action: DaemonAction) -> Result<()> {
    anyhow::bail!("`abyss daemon` is Unix-only on V1; use `abyss watch` on Windows");
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use crate::embedding::Embedder;
    use crate::indexer::IndexPipeline;
    use crate::storage::Repository;
    use crate::watcher::FileWatcher;
    use anyhow::Context;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    pub fn start(config: Config, foreground: bool, detach: bool) -> Result<()> {
        // Refuse early: a daemon against a non-existent index would spin
        // useless reindex loops at startup.
        if !config.db_path.exists() {
            anyhow::bail!(
                "no index found at {} — run `abyss index` first",
                config.db_path.display()
            );
        }

        let paths = DaemonPaths::from_config(&config);
        std::fs::create_dir_all(&paths.dir)
            .with_context(|| format!("create {}", paths.dir.display()))?;

        // --detach: double-fork + setsid before we touch any locks. The
        // parent process returns to the shell once the child has had a
        // moment to acquire the pidfile, so callers can synchronously chain
        // `daemon start --detach && daemon status` without a race.
        // Mutually exclusive with --foreground; --detach wins (an explicit
        // background flag overrides the "stay attached" default).
        if detach && !foreground {
            if let Some(_parent_done) = daemonize(&paths)? {
                // Parent path — return from this function (and from main).
                // The grandchild is now running independently.
                return Ok(());
            }
            // Grandchild: redirect happened inside daemonize(), continue.
        } else if !foreground {
            redirect_log(&paths.log)?;
        }

        // Pidfile lock — flock-based. Second instance fails fast with a clear
        // message rather than racing the watcher. (Acquired after the fork
        // so the parent doesn't briefly hold then release it.)
        let pidfile = PidFile::acquire(&paths.pid)
            .context("acquire daemon.pid — is another `abyss daemon` already running?")?;

        let state = Arc::new(DaemonState::new(
            std::process::id(),
            &config,
            paths.log.clone(),
        ));

        // Socket server in its own thread — we want the watcher loop free to
        // own the main thread (it polls a stop channel + signal flag).
        let socket_state = state.clone();
        let socket_path = paths.socket.clone();
        let socket_stop = Arc::new(AtomicBool::new(false));
        let socket_stop_for_thread = socket_stop.clone();
        let socket_handle = std::thread::spawn(move || {
            if let Err(e) = socket::serve(&socket_path, socket_state, socket_stop_for_thread) {
                tracing::warn!("daemon socket exited: {e}");
            }
        });

        // SIGINT/SIGTERM → set the shutdown flag. The watcher polls a stop
        // channel at POLL_TICK, so we tag both in one shot.
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        install_signal_handler(stop_tx.clone())?;

        // Open repo + pipeline + (optional) embedder.
        let repo = Repository::open(&config.db_path, config.model.dimensions)?;
        let pipeline = IndexPipeline::new(config.clone());

        #[cfg(feature = "semantic")]
        let embedder: Option<Embedder> = match Embedder::load(&config.model) {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::warn!("embedder unavailable ({e}) — structural only");
                None
            }
        };
        #[cfg(not(feature = "semantic"))]
        let embedder: Option<Embedder> = None;

        let watcher = FileWatcher::new(config.clone())
            .with_debounce(Duration::from_millis(crate::watcher::DEFAULT_DEBOUNCE_MS))
            .with_on_reindex({
                let st = state.clone();
                move |elapsed_ms| st.record_reindex(elapsed_ms)
            });

        tracing::info!(
            "abyss daemon started (pid {}, socket {})",
            std::process::id(),
            paths.socket.display()
        );

        // Watch until stop signal. The watcher itself owns this thread.
        let watch_result =
            watcher.watch_with_cancel(&repo, embedder.as_ref(), &pipeline, Some(stop_rx));

        // Cleanup order: signal socket thread, join it, drop pidfile guard
        // (which also unlinks). Socket-file removal happens inside serve().
        socket_stop.store(true, Ordering::SeqCst);
        // Nudge the accept() loop with a dummy connection so it observes the flag.
        let _ = std::os::unix::net::UnixStream::connect(&paths.socket);
        let _ = socket_handle.join();

        // PidFile drop unlinks the file.
        drop(pidfile);

        // Best-effort socket cleanup (serve() should already have unlinked).
        let _ = std::fs::remove_file(&paths.socket);

        tracing::info!("abyss daemon stopped cleanly");
        watch_result
    }

    pub fn stop(config: Config) -> Result<()> {
        let paths = DaemonPaths::from_config(&config);
        let pid = read_pid(&paths.pid).with_context(|| format!("read {}", paths.pid.display()))?;

        // SIGTERM is the polite shutdown — the daemon's signal handler will
        // flip the stop flag and the watcher/socket will tear down.
        // SAFETY: kill(2) with a valid signal and a real pid is a standard
        // POSIX syscall — no Rust-side invariants involved.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            // ESRCH = stale pidfile (process already gone). Clean it up.
            if err.raw_os_error() == Some(libc::ESRCH) {
                let _ = std::fs::remove_file(&paths.pid);
                let _ = std::fs::remove_file(&paths.socket);
                anyhow::bail!("no running daemon (pid {pid} not found — stale pidfile cleared)");
            }
            return Err(anyhow::Error::new(err).context(format!("kill({pid}, SIGTERM)")));
        }

        // Poll for pidfile removal up to 5s. The daemon's PidFile guard
        // unlinks on Drop, so disappearance is the canonical "stopped" signal.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !paths.pid.exists() {
                eprintln!("abyss daemon stopped (pid {pid})");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        anyhow::bail!(
            "daemon (pid {pid}) did not exit within 5s — send SIGKILL manually if needed"
        );
    }

    pub fn status(config: Config) -> Result<()> {
        let paths = DaemonPaths::from_config(&config);
        if !paths.pid.exists() {
            eprintln!("abyss daemon: not running");
            std::process::exit(1);
        }

        let pid = match read_pid(&paths.pid) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("abyss daemon: pidfile unreadable ({e})");
                std::process::exit(1);
            }
        };

        // SAFETY: kill(pid, 0) is the standard liveness probe — it sends no
        // signal, just returns 0 if a process with that pid exists and the
        // caller may signal it.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        if !alive {
            eprintln!("abyss daemon: stale pidfile (pid {pid} gone)");
            std::process::exit(1);
        }

        // Query the socket for uptime + last reindex. If the socket is gone or
        // unresponsive, fall back to "running, telemetry unavailable" rather
        // than failing the whole status call.
        let telemetry = socket::ping(&paths.socket).ok();
        match telemetry {
            Some(t) => println!(
                "abyss daemon: running\n  pid: {}\n  uptime: {}s\n  last reindex: {}ms ago\n  socket: {}",
                pid,
                t.uptime_secs,
                t.last_reindex_ms,
                paths.socket.display()
            ),
            None => println!(
                "abyss daemon: running\n  pid: {}\n  socket: {} (no response)",
                pid,
                paths.socket.display()
            ),
        }
        Ok(())
    }

    pub fn logs(config: Config, tail: usize, follow: bool) -> Result<()> {
        let paths = DaemonPaths::from_config(&config);
        if !paths.pid.exists() {
            // No running daemon — fall back to a direct read of the log file
            // so operators can still inspect the last session's output.
            if !paths.log.exists() {
                eprintln!(
                    "abyss daemon: no log at {} (daemon never ran here)",
                    paths.log.display()
                );
                std::process::exit(1);
            }
            print_tail(&paths.log, tail)?;
            if follow {
                follow_log(&paths.log)?;
            }
            return Ok(());
        }

        // Live daemon — go through the socket so the read stays consistent
        // with what the daemon thinks of as "its log" (matters once the
        // daemon learns about log rotation).
        let resp = match socket::logs(&paths.socket, tail) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("abyss daemon: socket unreachable ({e}); falling back to log file");
                print_tail(&paths.log, tail)?;
                if follow {
                    follow_log(&paths.log)?;
                }
                return Ok(());
            }
        };
        for line in resp.lines {
            println!("{line}");
        }
        // --follow path: stream new bytes appended to the log file directly.
        // This bypasses the socket because the daemon is the writer — the
        // log file is the canonical source. Polling is the simplest portable
        // strategy (inotify would need a feature gate); 200ms is fast enough
        // to feel live without burning CPU.
        if follow {
            follow_log(&paths.log)?;
        }
        Ok(())
    }

    /// Open the log file, seek to end, then poll for new bytes every
    /// 200ms and emit any that appeared since the last read. Returns when
    /// stdout closes (broken pipe) — that's how `Ctrl-C` exits cleanly when
    /// the user redirects through `head` or similar. SIGINT in the bare
    /// terminal case terminates the process before we read again.
    ///
    /// Handles two edge cases:
    /// 1. File rotation (size shrinks) — re-open and continue from byte 0.
    /// 2. File deleted then recreated — same as rotation; bounded retry.
    pub fn follow_log(log_path: &std::path::Path) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut file = match std::fs::File::open(log_path) {
            Ok(f) => f,
            Err(e) => {
                // File vanished between print_tail and follow — try once
                // more then bail. Most likely a race against `daemon stop`.
                std::thread::sleep(Duration::from_millis(200));
                std::fs::File::open(log_path).map_err(|_| {
                    anyhow::anyhow!("follow: open {} failed: {e}", log_path.display())
                })?
            }
        };
        let mut pos = file.seek(SeekFrom::End(0))?;
        let mut buf = [0u8; 8192];
        let stdout = std::io::stdout();
        loop {
            // Check for rotation by re-stat'ing.
            let meta = std::fs::metadata(log_path);
            if let Ok(m) = &meta
                && m.len() < pos
            {
                // Truncated — re-open from the top.
                file = std::fs::File::open(log_path)?;
                pos = 0;
            }
            file.seek(SeekFrom::Start(pos))?;
            loop {
                let n = match file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) => {
                        // Surface unexpected I/O — but EAGAIN/WOULDBLOCK
                        // shouldn't happen on a blocking File. Bail loudly.
                        anyhow::bail!("follow: read {} failed: {e}", log_path.display());
                    }
                };
                pos += n as u64;
                let mut out = stdout.lock();
                if out.write_all(&buf[..n]).is_err() {
                    // Broken pipe → caller closed stdout (e.g. `| head`).
                    // Exit cleanly without an angry error message.
                    return Ok(());
                }
                let _ = out.flush();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn print_tail(log_path: &std::path::Path, tail: usize) -> Result<()> {
        use std::collections::VecDeque;
        use std::io::BufRead;
        let file = std::fs::File::open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        let reader = std::io::BufReader::new(file);
        let mut buf: VecDeque<String> = VecDeque::with_capacity(tail);
        for line in reader.lines() {
            let line = line.unwrap_or_default();
            if buf.len() == tail {
                buf.pop_front();
            }
            buf.push_back(line);
        }
        for line in buf {
            println!("{line}");
        }
        Ok(())
    }

    fn read_pid(path: &std::path::Path) -> Result<u32> {
        let raw = std::fs::read_to_string(path)?;
        let pid: u32 = raw.trim().parse().context("pidfile contents not a u32")?;
        Ok(pid)
    }

    /// Double-fork + setsid daemonization.
    ///
    /// Returns `Ok(Some(()))` in the parent (or first child) — caller should
    /// return up to main. Returns `Ok(None)` in the final grandchild, which
    /// continues into the normal daemon loop. Returns `Err` on syscall
    /// failure in the parent before the first fork.
    ///
    /// Why double-fork: the first fork detaches us from the shell's process
    /// group, then `setsid()` makes the child a session leader. The second
    /// fork re-orphans us so the daemon is *not* a session leader, which
    /// prevents it from ever (re)acquiring a controlling terminal. This is
    /// the standard SysV daemonization recipe.
    ///
    /// Stdin is closed; stdout/stderr are dup2'd to `daemon.log` *after* the
    /// final fork so any pre-fork error still surfaces to the user's shell.
    fn daemonize(paths: &DaemonPaths) -> Result<Option<()>> {
        use std::os::unix::io::AsRawFd;

        // Pre-open the log file in the parent so a permission error here
        // surfaces to the user's shell, not the silent grandchild.
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.log)
            .with_context(|| format!("open {}", paths.log.display()))?;

        // SAFETY: fork(2) is a standard POSIX call; the only Rust-side
        // invariant is that we don't run destructors in the child that
        // assume parent-process state — we explicitly return Ok(Some(()))
        // from the parent path so RAII unwinds normally there.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(
                anyhow::Error::new(std::io::Error::last_os_error()).context("fork(1) failed")
            );
        }
        if pid > 0 {
            // Parent — wait briefly for the grandchild to claim the pidfile
            // so the caller's shell can chain `start --detach && status`
            // without a race. 500ms covers the cold-start + DB-open path on
            // realistic hardware.
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < deadline {
                if paths.pid.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            return Ok(Some(()));
        }

        // First child — become session leader so we shed the controlling tty.
        // SAFETY: setsid(2) is a standard POSIX call with no Rust-side
        // invariants. Failure here is a hard stop — we'd remain attached.
        if unsafe { libc::setsid() } < 0 {
            let err = std::io::Error::last_os_error();
            anyhow::bail!("setsid failed: {err}");
        }

        // Second fork — orphan ourselves so we can never re-acquire a tty.
        // SAFETY: same as the first fork.
        let pid2 = unsafe { libc::fork() };
        if pid2 < 0 {
            let err = std::io::Error::last_os_error();
            anyhow::bail!("fork(2) failed: {err}");
        }
        if pid2 > 0 {
            // First-child intermediary: exit so the grandchild gets reparented
            // to init. _exit avoids running atexit handlers / destructors that
            // the parent process still owns.
            // SAFETY: _exit(2) is a no-return POSIX call; nothing after this
            // line executes in this process.
            unsafe { libc::_exit(0) };
        }

        // Grandchild — the real daemon. Redirect stdio so any tracing or
        // accidental println! goes to the log file, not a possibly-gone tty.
        // SAFETY: dup2/close are standard POSIX calls; we keep `log_file`
        // alive (via mem::forget) so the underlying fd stays open beyond
        // this scope.
        unsafe {
            // /dev/null for stdin so reads return EOF instead of blocking.
            let devnull = libc::open(c"/dev/null".as_ptr() as *const libc::c_char, libc::O_RDONLY);
            if devnull >= 0 {
                libc::dup2(devnull, libc::STDIN_FILENO);
                libc::close(devnull);
            }
            let fd = log_file.as_raw_fd();
            // Header — matches the redirect_log() format so log readers can't
            // tell a --detach run apart from a backgrounded `start &` run.
            let header = format!(
                "--- abyss daemon pid={} start (detached) ---\n",
                libc::getpid()
            );
            let _ = libc::write(fd, header.as_ptr() as *const libc::c_void, header.len());
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::dup2(fd, libc::STDERR_FILENO);
        }
        std::mem::forget(log_file);
        Ok(None)
    }

    fn redirect_log(log_path: &std::path::Path) -> Result<()> {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        // Header so log readers can tell daemon runs apart.
        let mut f2 = f.try_clone()?;
        let _ = writeln!(f2, "--- abyss daemon pid={} start ---", std::process::id());

        // Re-init tracing into the log file. tracing_subscriber::fmt's init()
        // is idempotent-failing (returns Err if a subscriber is already set);
        // since main.rs already installed one, we route via stderr redirection
        // instead — point fd 2 at the log so all tracing output lands there.
        // SAFETY: dup2 swaps fd 2 to point at the log file; libc-blessed call,
        // no Rust-side invariants involved.
        use std::os::unix::io::AsRawFd;
        unsafe {
            let fd = f.as_raw_fd();
            if libc::dup2(fd, libc::STDERR_FILENO) < 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!("dup2(log, stderr) failed: {err}");
            }
        }
        // Hold the file open for the process lifetime — closing it after dup2
        // is fine (the kernel keeps the underlying open file), but explicitly
        // leaking makes intent obvious.
        std::mem::forget(f);
        Ok(())
    }

    fn install_signal_handler(stop_tx: std::sync::mpsc::Sender<()>) -> Result<()> {
        use std::sync::Mutex;
        static HANDLER_TX: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);
        *HANDLER_TX.lock().unwrap() = Some(stop_tx);

        extern "C" fn handle(_sig: libc::c_int) {
            // Best-effort: signal handlers can't allocate, can't take locks
            // without risking deadlock — so we use try_lock and quietly drop
            // the notification if we lose the race (caller is exiting anyway).
            if let Ok(guard) = HANDLER_TX.try_lock()
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.send(());
            }
        }

        // SAFETY: signal(2) with SIG_DFL-compatible function pointers is a
        // standard POSIX call. The handler above only calls async-signal-safe
        // operations (try_lock + send on an mpsc are not strictly AS-safe but
        // we accept the small risk in V1 — same trade-off ctrl-c crates make).
        unsafe {
            libc::signal(libc::SIGTERM, handle as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, handle as *const () as libc::sighandler_t);
        }
        Ok(())
    }
}
