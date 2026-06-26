//! Full output preservation ("tee") for debugging.
//!
//! When a proxied command fails (non-zero exit), the unfiltered output is
//! saved to `.code-abyss/tee/` so the agent can request the full log path
//! without re-running the command.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

const MAX_TEE_FILES: usize = 50;
const MIN_TEE_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum TeeMode {
    #[default]
    Failures,
    Always,
    Never,
}

pub fn should_tee(mode: TeeMode, exit_code: i32, output_len: usize) -> bool {
    if mode == TeeMode::Never {
        return false;
    }
    if output_len < MIN_TEE_SIZE {
        return false;
    }
    if std::env::var("ABYSS_TEE").as_deref() == Ok("0") {
        return false;
    }
    match mode {
        TeeMode::Failures => exit_code != 0,
        TeeMode::Always => true,
        TeeMode::Never => false,
    }
}

pub fn write_tee(tee_dir: &Path, command_slug: &str, content: &str) -> Result<Option<PathBuf>> {
    if content.is_empty() {
        return Ok(None);
    }

    fs::create_dir_all(tee_dir)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let slug: String = command_slug
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .take(60)
        .collect();

    let filename = format!("{ts}_{slug}.log");
    let path = tee_dir.join(&filename);
    fs::write(&path, content)?;

    cleanup_old(tee_dir);
    Ok(Some(path))
}

fn cleanup_old(tee_dir: &Path) {
    let mut entries: Vec<_> = fs::read_dir(tee_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("log"))
        .collect();

    if entries.len() <= MAX_TEE_FILES {
        return;
    }

    entries.sort_by_key(|e| e.path());
    let to_remove = entries.len() - MAX_TEE_FILES;
    for entry in entries.into_iter().take(to_remove) {
        let _ = fs::remove_file(entry.path());
    }
}

/// Format a tee hint line for inclusion in filtered output.
pub fn tee_hint(path: &Path) -> String {
    format!("[full output: {}]", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_tee_failures_mode() {
        assert!(should_tee(TeeMode::Failures, 1, 1000));
        assert!(!should_tee(TeeMode::Failures, 0, 1000));
        assert!(!should_tee(TeeMode::Failures, 1, 100));
    }

    #[test]
    fn should_tee_always_mode() {
        assert!(should_tee(TeeMode::Always, 0, 1000));
        assert!(should_tee(TeeMode::Always, 1, 1000));
    }

    #[test]
    fn should_tee_never_mode() {
        assert!(!should_tee(TeeMode::Never, 1, 10000));
    }
}
