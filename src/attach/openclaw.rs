//! `abyss attach openclaw` — intentional no-op with guidance.
//!
//! Reality check against the sister `code-abyss` package's adapter
//! (`bin/adapters/openclaw.js`): **OpenClaw does not consume hook
//! definitions from `~/.openclaw/config.toml`**. The production-tested
//! installer puts abyss integration into a per-pack directory layout
//! (`packs/abyss/openclaw/`), not into a shared settings file.
//!
//! Shipping a `config.toml` hook stanza from `abyss attach openclaw`
//! would write a file that OpenClaw never reads — which is worse than
//! useless, because the user thinks they've wired the hook when they
//! haven't. So this installer is deliberately downgraded:
//!
//! * `install()` returns an error with a clear migration message.
//! * `already_installed()` returns `false` so `attach all` won't tag the
//!   host as "already present".
//! * `settings_path()` still resolves the path so the per-host summary
//!   in `attach all` can mention the historical target.
//!
//! For real OpenClaw integration today, point users at:
//!
//! ```sh
//! npx code-abyss -t openclaw --with-abyss
//! ```
//!
//! or the in-tree pack at
//! `/home/telagod/project/code-abyss/packs/abyss/openclaw/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// Same path as before so `attach all`'s summary line keeps a stable
/// shape. The path is never written to — the install path bails out.
pub fn settings_path(local: bool) -> Result<PathBuf> {
    if local {
        let cwd = std::env::current_dir().context("cannot read current dir")?;
        return Ok(cwd.join(".openclaw").join("config.toml"));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
    })?;
    Ok(home.join(".openclaw").join("config.toml"))
}

/// Always `false`: there is nothing for us to install, so there is
/// nothing for us to be "already" installed as. Returning `false`
/// guarantees `attach all` doesn't claim success for this host.
pub fn already_installed(_path: &Path) -> bool {
    false
}

/// Always errors with the migration message. Both `attach openclaw` and
/// `attach all` route here.
pub fn install(_local: bool) -> Result<()> {
    bail_with_message()
}

/// Same shape as the other adapters' `install_at` so integration tests
/// and `attach all` can call it uniformly.
pub fn install_at(_path: &Path) -> Result<()> {
    bail_with_message()
}

fn bail_with_message() -> Result<()> {
    Err(anyhow!(
        "abyss attach openclaw: OpenClaw uses a per-pack install layout, not a settings file. \
         The sister code-abyss adapter installs abyss into packs/abyss/openclaw/, which `abyss attach` \
         cannot replicate from a single binary. \
         Use `npx code-abyss -t openclaw --with-abyss` instead, or copy /home/telagod/project/code-abyss/packs/abyss/openclaw/ manually. \
         This downgrade is intentional — see CHANGELOG v0.5.23."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_at_returns_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".openclaw/config.toml");
        let err = install_at(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("per-pack install layout"),
            "error message must explain why we refuse: {msg}"
        );
        assert!(
            msg.contains("npx code-abyss"),
            "error message must point at the working alternative: {msg}"
        );
        // And we must NOT have written the file.
        assert!(!path.exists(), "openclaw downgrade must not create files");
    }

    #[test]
    fn install_returns_error() {
        let err = install(true).unwrap_err();
        assert!(format!("{err}").contains("OpenClaw uses a per-pack install layout"));
    }

    #[test]
    fn already_installed_is_always_false() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        assert!(!already_installed(&path));
        // Even if the file exists with hooks-shaped content — still false.
        std::fs::write(
            &path,
            "[hooks.PreToolUse]\ncommand = \"abyss hook pre-edit\"\n",
        )
        .unwrap();
        assert!(!already_installed(&path));
    }

    #[test]
    fn settings_path_local_is_cwd_relative() {
        let p = settings_path(true).unwrap();
        assert!(p.is_absolute());
        let tail: std::path::PathBuf = p
            .iter()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .iter()
            .rev()
            .collect();
        assert_eq!(
            tail,
            std::path::PathBuf::from(".openclaw").join("config.toml")
        );
    }
}
