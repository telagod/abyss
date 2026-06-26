//! Subprocess execution and output capture for `abyss proxy`.
//!
//! Uses bounded capture: output is read incrementally and capped at
//! `MAX_CAPTURE_BYTES`. Excess output is noted but not held in memory.
//!
//! stdout and stderr are captured concurrently on separate threads to
//! prevent deadlocks when the child writes to one pipe while the other
//! is full (the classic deadlock on sequential reads).

use std::io::Read;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// 10 MiB capture limit per stream. Commands producing more are truncated
/// with a note; the full output goes to tee if enabled.
const MAX_CAPTURE_BYTES: usize = 10 * 1024 * 1024;

pub struct CapturedRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub command: String,
    pub truncated: bool,
}

impl CapturedRun {
    /// Build the combined raw output without cloning when only one stream
    /// has data (the common case — most commands write to either stdout
    /// or stderr, not both).
    pub fn raw_output(&self) -> RawOutput<'_> {
        if self.stderr.is_empty() {
            RawOutput::Borrowed(&self.stdout)
        } else if self.stdout.is_empty() {
            RawOutput::Borrowed(&self.stderr)
        } else {
            RawOutput::Owned(format!("{}\n{}", self.stdout, self.stderr))
        }
    }
}

/// Zero-copy wrapper: avoids allocating a new String when only one
/// stream has data (>90% of real-world commands).
pub enum RawOutput<'a> {
    Borrowed(&'a str),
    Owned(String),
}

impl<'a> RawOutput<'a> {
    pub fn as_str(&self) -> &str {
        match self {
            RawOutput::Borrowed(s) => s,
            RawOutput::Owned(s) => s,
        }
    }
}

impl<'a> std::ops::Deref for RawOutput<'a> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

pub fn run_captured(program: &str, args: &[String]) -> Result<CapturedRun> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn: {program}"))?;

    // Capture stdout and stderr on separate threads to prevent deadlocks.
    // If we read stdout first and the child fills the stderr pipe buffer
    // (~64KB on Linux), the child blocks on its stderr write and we block
    // waiting for stdout EOF — classic deadlock.
    let stdout_pipe = child.stdout.take().expect("piped");
    let stderr_pipe = child.stderr.take().expect("piped");

    let stdout_handle = std::thread::spawn(move || read_bounded(stdout_pipe, MAX_CAPTURE_BYTES));
    let (stderr, stderr_truncated) = read_bounded(stderr_pipe, MAX_CAPTURE_BYTES);

    let (stdout, stdout_truncated) = stdout_handle
        .join()
        .unwrap_or_else(|_| (String::new(), false));

    let status = child
        .wait()
        .with_context(|| format!("waiting for: {program}"))?;
    let exit_code = status.code().unwrap_or(1);
    let truncated = stdout_truncated || stderr_truncated;

    let command = if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    };

    Ok(CapturedRun {
        stdout,
        stderr,
        exit_code,
        command,
        truncated,
    })
}

fn read_bounded(mut reader: impl Read, limit: usize) -> (String, bool) {
    let mut buf = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0u8; 8192];
    let mut total = 0usize;
    let mut truncated = false;

    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let remaining = limit.saturating_sub(total);
                if remaining == 0 {
                    truncated = true;
                    // Drain the rest so the child doesn't block on a full pipe
                    loop {
                        match reader.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(_) => continue,
                            Err(_) => break,
                        }
                    }
                    break;
                }
                let take = n.min(remaining);
                buf.extend_from_slice(&chunk[..take]);
                total += n;
                if total > limit {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&buf).into_owned();
    (text, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_output_borrows_single_stream() {
        let run = CapturedRun {
            stdout: "hello".into(),
            stderr: String::new(),
            exit_code: 0,
            command: "echo".into(),
            truncated: false,
        };
        let raw = run.raw_output();
        assert_eq!(raw.as_str(), "hello");
        assert!(matches!(raw, RawOutput::Borrowed(_)));
    }

    #[test]
    fn raw_output_combines_both_streams() {
        let run = CapturedRun {
            stdout: "out".into(),
            stderr: "err".into(),
            exit_code: 0,
            command: "cmd".into(),
            truncated: false,
        };
        let raw = run.raw_output();
        assert_eq!(raw.as_str(), "out\nerr");
        assert!(matches!(raw, RawOutput::Owned(_)));
    }

    #[test]
    fn run_captured_echo() {
        let result = run_captured("echo", &["hello".into()]).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("hello"));
        assert!(!result.truncated);
    }

    #[test]
    fn run_captured_false_exits_nonzero() {
        let result = run_captured("false", &[]).unwrap();
        assert_ne!(result.exit_code, 0);
    }

    #[test]
    fn concurrent_capture_no_deadlock() {
        // Verify that writing a lot to stderr doesn't deadlock.
        // `sh -c` writes 200KB to stderr — well over the pipe buffer.
        let script = "dd if=/dev/zero bs=1024 count=200 status=none >&2; echo ok";
        let result = run_captured("sh", &["-c".into(), script.into()]).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("ok"));
        assert!(!result.stderr.is_empty());
    }
}
