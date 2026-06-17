//! `abyss daemon logs --follow` smoke test.
//!
//! Path under test: when no daemon is running we tail the log file directly
//! (no socket round-trip), seek to end, and stream new bytes as they're
//! appended. This is the same code path the CLI uses against a live daemon
//! — the daemon is the writer, the file is the source of truth, so `--follow`
//! skips the socket on purpose.
//!
//! Linux-only — matches the rest of the daemon test suite (`UnixListener` /
//! inotify are POSIX-shaped).

#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

/// Append a single line + newline to a daemon.log fixture.
fn append_line(path: &std::path::Path, line: &str) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open daemon.log");
    writeln!(f, "{line}").expect("append line");
    f.flush().ok();
}

/// Spawn `abyss daemon logs --follow --tail 0` against a workspace whose
/// `.code-abyss/daemon.log` exists but whose `daemon.pid` doesn't (no live
/// daemon). Append a line after the child has had a moment to start, then
/// assert the line shows up on the child's stdout within 1.5s.
///
/// We deliberately use a small `--tail` so the initial snapshot stays out
/// of the way and the assertion focuses on the streamed line.
#[test]
fn follow_streams_appended_line_to_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = std::fs::canonicalize(dir.path()).expect("canonicalize");
    let abyss_dir = ws.join(".code-abyss");
    std::fs::create_dir_all(&abyss_dir).expect("mkdir .code-abyss");

    let log_path = abyss_dir.join("daemon.log");
    // Seed the log so the path exists and `--tail` has a baseline to skip
    // over. The streamed line below is the actual assertion target.
    append_line(&log_path, "SEED LINE 0");
    append_line(&log_path, "SEED LINE 1");
    append_line(&log_path, "SEED LINE 2");

    // No pidfile, no socket: the CLI takes the "no running daemon" branch,
    // prints the tail directly, then enters follow_log() on the file.
    let mut child = Command::new(abyss_binary())
        .arg("--workspace")
        .arg(&ws)
        .arg("daemon")
        .arg("logs")
        .arg("--tail")
        .arg("0")
        .arg("--follow")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn abyss daemon logs --follow");

    // Give the child time to open the file and seek to end. 200ms is
    // generous — Linux file open is sub-ms.
    std::thread::sleep(Duration::from_millis(300));

    // Append a fresh line. The follow loop polls every 200ms, so we should
    // see this on stdout within ~500ms.
    let needle = "STREAMED LINE AFTER FOLLOW STARTED";
    append_line(&log_path, needle);

    // Read child stdout in a background thread so we can apply a hard
    // deadline without blocking on read().
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = [0u8; 4096];
        let mut acc = String::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if acc.contains("STREAMED LINE AFTER FOLLOW STARTED") {
                        let _ = tx.send(acc.clone());
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(acc);
    });

    let observed = rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or_else(|_| String::from("<no stdout received>"));

    // Tear down the child before asserting — failures shouldn't leave a
    // background process polling a tempdir.
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        observed.contains(needle),
        "follow did not stream the appended line within 3s.\nobserved stdout:\n{observed}"
    );
}
