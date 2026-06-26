//! Subprocess execution and output capture for `abyss proxy`.

use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result};

/// Captured output from a proxied command.
pub struct CapturedRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub command: String,
}

/// Run a command, capture its stdout/stderr, return structured result.
pub fn run_captured(program: &str, args: &[String]) -> Result<CapturedRun> {
    let output: Output = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to spawn: {program}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(1);

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
    })
}

/// Run command and return only stdout as string (convenience).
pub fn run_stdout(program: &str, args: &[String]) -> Result<String> {
    let result = run_captured(program, args)?;
    Ok(result.stdout)
}
