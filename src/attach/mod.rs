//! `abyss attach <host>` — install agent-side hooks idempotently.
//!
//! **Production main entrypoint** for the three hosts whose hook surface
//! is a single shared settings file. Each host has its own layout; the
//! shared contract is that re-running `attach` against an already-configured
//! host must be a no-op (no duplicate entries, no clobbered unrelated keys).
//!
//! ## 6-host responsibility split (architectural decision, stable as of v0.5.24)
//!
//! `abyss attach` owns the hosts whose hook config lives in a single shared
//! settings file. The companion `code-abyss` npm package owns the hosts
//! whose hook config either needs a per-pack layout or a still-evolving
//! shape. These are **not** temporary gaps — they're a deliberate split:
//!
//! - [`claude`]    — `~/.claude/settings.json` — **abyss attach (production)**
//! - [`codex`]     — `~/.codex/config.toml` (two-level array tables) — **abyss attach (production)**
//! - [`gemini`]    — `~/.gemini/settings.json` (`SessionStart`/`BeforeTool`/`AfterTool`) — **abyss attach (production)**
//! - [`openclaw`]  — per-pack layout `packs/abyss/openclaw/` — **delegated to `code-abyss`**.
//!   A single static binary cannot reliably create the per-pack directory
//!   tree, so `abyss attach openclaw` is an intentional no-op that errors
//!   with a migration message.
//! - Pi & Hermes — hook-config shapes still evolving across versions —
//!   **delegated to `code-abyss`**, where shape adapters can iterate
//!   independently of abyss's release cadence.
//!
//! See the `code-abyss` README for the openclaw/pi/hermes installer flow.

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod openclaw;

/// Canonical list of supported host slugs, in stable display order.
pub const SUPPORTED_HOSTS: &[&str] = &["claude", "codex", "gemini", "openclaw"];

/// Single dispatch entrypoint used by `cmd_attach` and `attach all`.
///
/// Returns a short status line (`"installed"` / `"already present"`) on
/// success so callers can render a per-host summary when running
/// `attach all`.
pub fn install_host(host: &str, local: bool) -> anyhow::Result<&'static str> {
    let path = match host {
        "claude" => {
            let already = claude_already_installed(local)?;
            claude::install(local)?;
            return Ok(if already {
                "already present"
            } else {
                "installed"
            });
        }
        "codex" => codex::settings_path(local)?,
        "gemini" => gemini::settings_path(local)?,
        "openclaw" => openclaw::settings_path(local)?,
        other => anyhow::bail!(
            "unknown host: {other}; supported: {}",
            SUPPORTED_HOSTS.join(", ")
        ),
    };

    let already = match host {
        "codex" => codex::already_installed(&path),
        "gemini" => gemini::already_installed(&path),
        "openclaw" => openclaw::already_installed(&path),
        _ => unreachable!(),
    };

    match host {
        "codex" => codex::install(local)?,
        "gemini" => gemini::install(local)?,
        "openclaw" => openclaw::install(local)?,
        _ => unreachable!(),
    }

    Ok(if already {
        "already present"
    } else {
        "installed"
    })
}

/// Claude's installer doesn't currently expose `already_installed`, so
/// we re-derive it from the settings file. Best-effort: a malformed
/// existing file is treated as "not installed" so `install_host` can
/// upgrade it.
fn claude_already_installed(local: bool) -> anyhow::Result<bool> {
    use std::path::PathBuf;

    let path: PathBuf = if local {
        std::env::current_dir()?
            .join(".claude")
            .join("settings.json")
    } else {
        let home = dirs::home_dir().ok_or_else(|| {
            anyhow::anyhow!("could not determine home directory (HOME / USERPROFILE not set)")
        })?;
        home.join(".claude").join("settings.json")
    };
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&path)?;
    if raw.trim().is_empty() {
        return Ok(false);
    }
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let has = |event: &str, cmd: &str| -> bool {
        v.get("hooks")
            .and_then(|h| h.get(event))
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter().any(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|i| i.as_array())
                        .map(|inner| {
                            inner
                                .iter()
                                .any(|h| h.get("command").and_then(|c| c.as_str()) == Some(cmd))
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    };
    Ok(has("PreToolUse", "abyss hook pre-edit") && has("PostToolUse", "abyss hook post-edit"))
}

/// Per-host result for `attach all`.
pub struct AttachResult {
    pub host: &'static str,
    /// `Ok((status, path_str))` on success — status is `"installed"` or `"already present"`.
    /// `Err(err)` when the host is skipped (parent dir missing) or fails outright.
    pub outcome: Result<(&'static str, String), String>,
}

/// Install every supported host. Hosts whose parent directory does not
/// exist (e.g. user never ran that agent) are reported as `skipped`.
///
/// `--local` writes into `<cwd>/.<host>/` for every host, which is
/// useful for testing. In that mode no host is skipped.
pub fn install_all(local: bool) -> Vec<AttachResult> {
    let mut results = Vec::new();
    for &host in SUPPORTED_HOSTS {
        results.push(install_one_for_all(host, local));
    }
    results
}

fn install_one_for_all(host: &'static str, local: bool) -> AttachResult {
    // OpenClaw is intentionally a no-op in v0.5.23+ (see attach/openclaw.rs).
    // We surface that to `attach all` as a "skipped: …" line so re-running
    // never tags the host as installed and never fails the batch.
    if host == "openclaw" {
        return AttachResult {
            host,
            outcome: Err(
                "skipped: openclaw uses a per-pack install layout; use `npx code-abyss -t openclaw --with-abyss`"
                    .to_string(),
            ),
        };
    }

    // Resolve the settings path first so we can both report it and decide
    // whether to skip (home-mode only).
    let path_res: anyhow::Result<std::path::PathBuf> = match host {
        "claude" => {
            if local {
                std::env::current_dir()
                    .map(|cwd| cwd.join(".claude").join("settings.json"))
                    .map_err(Into::into)
            } else {
                dirs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("no home directory"))
                    .map(|h| h.join(".claude").join("settings.json"))
            }
        }
        "codex" => codex::settings_path(local),
        "gemini" => gemini::settings_path(local),
        "openclaw" => openclaw::settings_path(local),
        _ => unreachable!(),
    };
    let path = match path_res {
        Ok(p) => p,
        Err(e) => {
            return AttachResult {
                host,
                outcome: Err(format!("path resolution failed: {e}")),
            };
        }
    };

    // Skip when the host's parent dir doesn't exist in home mode — the
    // user hasn't installed that agent, so creating a stray dotfolder
    // would be noise.
    if !local
        && let Some(parent) = path.parent()
        && !parent.exists()
    {
        return AttachResult {
            host,
            outcome: Err(format!("skipped: {} does not exist", parent.display())),
        };
    }

    match install_host(host, local) {
        Ok(status) => AttachResult {
            host,
            outcome: Ok((status, path.display().to_string())),
        },
        Err(e) => AttachResult {
            host,
            outcome: Err(e.to_string()),
        },
    }
}
