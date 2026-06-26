//! Subprocess execution and output capture for `abyss proxy`.
//!
//! Uses bounded capture: output is read incrementally and capped at
//! `MAX_CAPTURE_BYTES`. Excess output is noted but not held in memory.

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

pub fn run_captured(program: &str, args: &[String]) -> Result<CapturedRun> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn: {program}"))?;

    // Read stdout with bound
    let (stdout, stdout_truncated) = read_bounded(
        child.stdout.take().expect("piped"),
        MAX_CAPTURE_BYTES,
    );

    // Read stderr with bound
    let (stderr, stderr_truncated) = read_bounded(
        child.stderr.take().expect("piped"),
        MAX_CAPTURE_BYTES,
    );

    let status = child.wait().with_context(|| format!("waiting for: {program}"))?;
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
