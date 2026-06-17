//! Single-instance pidfile lock via `flock(LOCK_EX | LOCK_NB)`.
//!
//! Why `flock` over a plain pidfile-then-read-the-pid scheme: file locks are
//! released automatically when the process dies (or the fd is closed), so a
//! kill-9'd daemon doesn't leave a stale lock — the next `daemon start`
//! reclaims it cleanly. We still write the pid to the file as a *contents*
//! payload for `daemon status` / `daemon stop` to read.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub struct PidFile {
    path: PathBuf,
    file: Option<File>,
}

impl PidFile {
    /// Open the pidfile, attempt an exclusive non-blocking flock, and write
    /// the current pid. Returns Err if another process holds the lock.
    pub fn acquire(path: &Path) -> Result<Self> {
        // Open create-or-truncate-on-write; we never want a stale pid string
        // sticking around after we win the lock.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;

        // SAFETY: flock(2) with a valid fd is a standard POSIX call. The fd
        // is owned by `file` which we keep alive for the guard's lifetime.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                anyhow::bail!(
                    "pidfile {} is locked by another process — daemon already running?",
                    path.display()
                );
            }
            return Err(anyhow::Error::new(err).context("flock pidfile"));
        }

        // Truncate + write the pid. We delayed the truncate until after the
        // lock so a concurrent reader can't see an empty file mid-handoff.
        file.set_len(0)?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.flush()?;

        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
        })
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        // Unlink first, then drop the fd (which releases the flock). Order
        // matters only loosely — if we crash between, the next daemon will
        // observe a stale pidfile but reclaim the lock anyway.
        let _ = std::fs::remove_file(&self.path);
        // Close fd → release lock.
        self.file.take();
    }
}
