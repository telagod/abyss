//! `abyss completion <shell>` smoke test.
//!
//! Verifies the completion subcommand emits a recognizable script for each
//! supported shell. We don't try to exec the script — that would require a
//! live shell in CI — we just check the well-known function or directive
//! names that prove clap_complete actually wrote a script.

use std::path::PathBuf;
use std::process::Command;

fn abyss_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_abyss"))
}

fn run_completion(shell: &str) -> (i32, String, String) {
    let out = Command::new(abyss_binary())
        .arg("completion")
        .arg(shell)
        .output()
        .expect("spawn abyss completion");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn bash_completion_contains_abyss_function() {
    let (code, stdout, stderr) = run_completion("bash");
    assert_eq!(code, 0, "bash completion exit nonzero: stderr={stderr}");
    // clap_complete bash output starts with the function declaration
    // `_abyss()` and defines the `complete -F _abyss abyss` registration.
    assert!(
        stdout.contains("_abyss()"),
        "bash output missing `_abyss()`; first 200 bytes:\n{}",
        &stdout[..stdout.len().min(200)]
    );
    assert!(
        stdout.contains("complete -F"),
        "bash output missing `complete -F` registration"
    );
}

#[test]
fn zsh_completion_contains_abyss_function() {
    let (code, stdout, stderr) = run_completion("zsh");
    assert_eq!(code, 0, "zsh completion exit nonzero: stderr={stderr}");
    // clap_complete zsh output uses `#compdef abyss` header and defines
    // `_abyss` function. Pin both to catch silent format regressions.
    assert!(
        stdout.contains("#compdef abyss"),
        "zsh output missing `#compdef abyss` header"
    );
    assert!(stdout.contains("_abyss"), "zsh output missing _abyss");
}

#[test]
fn fish_completion_contains_abyss_command() {
    let (code, stdout, stderr) = run_completion("fish");
    assert_eq!(code, 0, "fish completion exit nonzero: stderr={stderr}");
    // fish completion uses `complete -c abyss` per line.
    assert!(
        stdout.contains("complete -c abyss"),
        "fish output missing `complete -c abyss`"
    );
}

#[test]
fn powershell_completion_contains_abyss_block() {
    let (code, stdout, stderr) = run_completion("powershell");
    assert_eq!(
        code, 0,
        "powershell completion exit nonzero: stderr={stderr}"
    );
    // powershell output declares `Register-ArgumentCompleter` and references
    // the binary name `abyss`.
    assert!(
        stdout.contains("Register-ArgumentCompleter"),
        "powershell output missing Register-ArgumentCompleter"
    );
    assert!(
        stdout.contains("abyss"),
        "powershell output missing `abyss` binary reference"
    );
}
