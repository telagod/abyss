//! Background daemon V1 — single-instance pidfile lock + Unix-socket
//! request/response loop in front of the existing [`FileWatcher`].
//!
//! V1 scope (intentionally narrow):
//! - `abyss daemon start [--foreground]` — claim pidfile, bind socket, watch.
//! - `abyss daemon stop` — SIGTERM the recorded pid, wait up to 5s for cleanup.
//! - `abyss daemon status` — print pid + uptime + last reindex + socket path.
//! - Protocol: newline-delimited JSON. Two verbs — `ping`, `stats`. The full
//!   MCP-over-socket surface is V2 territory.
//!
//! Backgrounding: V1 does *not* double-fork. Users run
//! `abyss daemon start &` (or use a service manager). We only own the
//! pidfile, the socket, the log file, and clean shutdown — keeping the
//! single-binary install story honest.
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
    Start { foreground: bool },
    Stop,
    Status,
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
        DaemonAction::Start { foreground } => unix_impl::start(config, foreground),
        DaemonAction::Stop => unix_impl::stop(config),
        DaemonAction::Status => unix_impl::status(config),
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

    pub fn start(config: Config, foreground: bool) -> Result<()> {
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

        // Pidfile lock — flock-based. Second instance fails fast with a clear
        // message rather than racing the watcher.
        let pidfile = PidFile::acquire(&paths.pid)
            .context("acquire daemon.pid — is another `abyss daemon` already running?")?;

        // Redirect tracing to the log file unless --foreground. We append so
        // restarts don't clobber prior runs.
        if !foreground {
            redirect_log(&paths.log)?;
        }

        let state = Arc::new(DaemonState::new(std::process::id(), &config));

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

    fn read_pid(path: &std::path::Path) -> Result<u32> {
        let raw = std::fs::read_to_string(path)?;
        let pid: u32 = raw.trim().parse().context("pidfile contents not a u32")?;
        Ok(pid)
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
