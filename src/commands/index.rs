use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::indexer::IndexPipeline;
use crate::storage::Repository;

pub const DEFAULT_MAX_FILES_NO_GIT: u64 = 50_000;

/// Indexable file extensions for the `--since` driver. Keep in sync with
/// `walker::is_indexable` — the goal is to pre-filter git's diff output
/// to what abyss would actually parse, so we don't spawn the pipeline
/// against `.png` or `.lock` files that the walker would silently drop
/// anyway. Out-of-sync is a soft failure (extra parse-then-skip), not
/// a correctness bug.
pub const INDEXABLE_EXTS: &[&str] = &[
    "rs", "py", "pyi", "js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx", "go", "java", "c",
    "h", "cpp", "cc", "cxx", "hpp", "hxx", "hh", "json", "toml", "yml", "yaml", "sh", "bash",
    "html", "htm", "css", "scss", "md", "markdown",
];

/// Scope of an `abyss reset` invocation. The CLI flags map onto these
/// branches once — keep the per-mode path resolution in `reset_targets`
/// so dry-run and the actual unlink stay byte-identical.
#[derive(Clone, Copy)]
pub enum ResetScope {
    /// Default: remove the index DB only, preserve `arch.toml` and any
    /// hand-authored config. The operator's intent is "rebuild the index".
    Default,
    /// `--all`: nuke the entire `.code-abyss/` directory. True greenfield.
    All,
    /// `--daemon`: remove daemon.pid / daemon.sock / daemon.log only.
    Daemon,
}

pub fn check_workspace_safety(workspace: &Path, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let home = std::env::var("HOME")
        .ok()
        .and_then(|h| std::fs::canonicalize(h).ok());
    let banned: Vec<std::path::PathBuf> = [home, Some(std::path::PathBuf::from("/"))]
        .into_iter()
        .flatten()
        .collect();
    for b in &banned {
        if workspace == b {
            anyhow::bail!(
                "refusing to index {} (too broad). Use --force to override, or --workspace to target a project directory.",
                workspace.display()
            );
        }
    }
    Ok(())
}

/// Run `git diff --name-only --diff-filter=...` twice (added/modified/renamed
/// vs deleted) against `<since>..HEAD` and split the output. Returns
/// `(changed_abs_paths, deleted_rel_paths)` already filtered to indexable
/// extensions so the caller doesn't have to.
pub fn git_diff_changeset(
    workspace: &Path,
    since: &str,
) -> Result<(Vec<std::path::PathBuf>, Vec<String>)> {
    use std::process::Command;
    let range = format!("{since}..HEAD");

    let run = |filter: &str| -> Result<Vec<String>> {
        let out = Command::new("git")
            .current_dir(workspace)
            .args([
                "diff",
                "--name-only",
                &format!("--diff-filter={filter}"),
                &range,
            ])
            .output()
            .with_context(|| format!("git diff --diff-filter={filter}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "git diff exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    };

    let is_indexable = |p: &str| {
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| INDEXABLE_EXTS.contains(&ext))
    };

    // AMR: added / modified / renamed → reindex from the worktree.
    let changed_rel = run("AMR")?;
    let changed: Vec<std::path::PathBuf> = changed_rel
        .into_iter()
        .filter(|p| is_indexable(p))
        .map(|p| workspace.join(p))
        .collect();

    // D: deleted → drop from the index. Keep these workspace-relative so
    // they match the `files.path` column directly.
    let deleted: Vec<String> = run("D")?.into_iter().filter(|p| is_indexable(p)).collect();

    Ok((changed, deleted))
}

pub fn cmd_index(
    mut config: Config,
    json: bool,
    force: bool,
    max_files: Option<u64>,
    index_generated: bool,
    since: Option<String>,
) -> Result<()> {
    check_workspace_safety(&config.workspace, force)?;
    config.index.index_generated = index_generated;

    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let mut pipeline = IndexPipeline::new(config.clone());

    let has_git = config.workspace.join(".git").exists();
    let limit = match max_files {
        Some(0) => None,
        Some(n) => Some(n),
        None if force || has_git => None,
        None => Some(DEFAULT_MAX_FILES_NO_GIT),
    };
    if let Some(n) = limit {
        pipeline.set_max_files(n);
    }

    // `--since <ref>`: ask git for the change set. Falls back to a full
    // walk (with a stderr warning) when the workspace isn't a git repo —
    // the operator's intent is "index", not "fail on a missing optimization".
    if let Some(ref reference) = since {
        if !has_git {
            eprintln!(
                "abyss index --since: workspace is not a git repo — falling back to full walk"
            );
        } else {
            match git_diff_changeset(&config.workspace, reference) {
                Ok((changed, deleted)) => {
                    eprintln!(
                        "abyss index --since {reference}: {} changed, {} deleted",
                        changed.len(),
                        deleted.len()
                    );
                    pipeline.set_restrict_paths(changed, deleted);
                }
                Err(e) => {
                    // A bad ref shouldn't silently morph into a full walk —
                    // the operator picked --since deliberately. Surface and
                    // bail so they catch a typo before it eats CPU.
                    anyhow::bail!("abyss index --since {reference}: {e}");
                }
            }
        }
    }

    let stats = pipeline.run_structural(&repo)?;

    if json {
        println!("{}", serde_json::to_string(&stats)?);
    } else {
        eprintln!(
            "✓ {} files, {} chunks, {} symbols, {} refs in {}ms",
            stats.total_files,
            stats.total_chunks,
            stats.total_symbols,
            stats.refs,
            stats.duration_ms
        );
    }
    Ok(())
}

#[cfg(feature = "semantic")]
pub fn cmd_embed(config: Config) -> Result<()> {
    use crate::embedding::Embedder;
    use tracing::info;

    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config);

    let stats = pipeline.run_embedding(&repo, &embedder)?;

    eprintln!(
        "✓ embedded {} chunks, skipped {}, in {:.1}s",
        stats.embedded,
        stats.skipped,
        stats.duration_ms as f64 / 1000.0
    );

    Ok(())
}

#[cfg(feature = "semantic")]
pub fn cmd_index_all(config: Config, watch: bool) -> Result<()> {
    use crate::embedding::Embedder;
    use tracing::info;

    info!("loading model: {}", config.model.model_id);
    let embedder = Embedder::load(&config.model)?;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let pipeline = IndexPipeline::new(config.clone());

    let stats = pipeline.run(&repo, &embedder)?;

    eprintln!(
        "✓ {} files, {} chunks, {} symbols in {:.1}s (embed: {:.1}s)",
        stats.total_files,
        stats.total_chunks,
        stats.total_symbols,
        stats.duration_ms as f64 / 1000.0,
        stats.embed_duration_ms as f64 / 1000.0
    );

    if watch {
        let watcher = crate::watcher::FileWatcher::new(config);
        watcher.watch(&repo, Some(&embedder), &pipeline)?;
    }

    Ok(())
}

/// Returns the live daemon pid if one is running against this workspace.
/// A pidfile without a backing process is *not* live — we want operators
/// to be able to `abyss reset` after an `kill -9` left a stale pid behind.
pub fn live_daemon_pid(config: &Config) -> Option<u32> {
    #[cfg(unix)]
    {
        use crate::daemon::DaemonPaths;
        let paths = DaemonPaths::from_config(config);
        if !paths.pid.exists() {
            return None;
        }
        let pid: u32 = std::fs::read_to_string(&paths.pid)
            .ok()?
            .trim()
            .parse()
            .ok()?;
        // SAFETY: `kill(pid, 0)` is the standard liveness probe — no signal
        // delivered, just a permission/existence check via the kernel.
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
        alive.then_some(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        None
    }
}

/// Resolve every filesystem path implicated by `scope`. Pure function so
/// the dry-run output and the actual removal traverse the identical list —
/// there can't be a target the unlink path forgets that dry-run promised.
fn reset_targets(config: &Config, scope: ResetScope) -> Vec<std::path::PathBuf> {
    let abyss_dir = config.workspace.join(".code-abyss");
    match scope {
        ResetScope::All => vec![abyss_dir],
        ResetScope::Default => {
            // The DB path is configurable (CLI `--db`), but the in-tree
            // default lives next to arch.toml. We rely on the resolved
            // `config.db_path` so a custom --db still gets nuked.
            vec![config.db_path.clone()]
        }
        ResetScope::Daemon => {
            vec![
                abyss_dir.join("daemon.pid"),
                abyss_dir.join("daemon.sock"),
                abyss_dir.join("daemon.log"),
            ]
        }
    }
}

fn scope_label(scope: ResetScope) -> &'static str {
    match scope {
        ResetScope::Default => "index-db",
        ResetScope::All => "all",
        ResetScope::Daemon => "daemon",
    }
}

pub fn cmd_reset(config: Config, all: bool, daemon: bool, dry_run: bool, json: bool) -> Result<()> {
    let scope = match (all, daemon) {
        (true, _) => ResetScope::All,
        (false, true) => ResetScope::Daemon,
        (false, false) => ResetScope::Default,
    };

    // Refuse while a daemon is alive — otherwise the running process would
    // recreate `daemon.sock` / `daemon.pid` mid-reset and operators end up
    // chasing ghosts. A stale pidfile (process gone) is *not* live and
    // counts as a normal cleanup target.
    if let Some(pid) = live_daemon_pid(&config) {
        anyhow::bail!(
            "abyss reset: daemon still running (pid {pid}). Stop it first: abyss daemon stop"
        );
    }

    let targets = reset_targets(&config, scope);
    // Filter to paths that actually exist so the output reads like a real
    // diff — "would remove" / "removed" on a missing file is noise.
    let existing: Vec<&std::path::PathBuf> = targets.iter().filter(|p| p.exists()).collect();

    if existing.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({"removed": [], "dry_run": dry_run, "scope": scope_label(scope)})
            );
        } else {
            eprintln!("abyss reset: nothing to remove ({})", scope_label(scope));
        }
        return Ok(());
    }

    let mut removed: Vec<String> = Vec::new();
    for path in &existing {
        let display = path.display().to_string();
        if dry_run {
            if !json {
                println!("would remove: {display}");
            }
        } else {
            // Directory vs file: `.code-abyss/` is a dir; everything else
            // we own is a regular file. Skipping the dir-detect probe would
            // make `--all` fail with "Is a directory" on the unlink call.
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(_) => {
                    if !json {
                        println!("removed: {display}");
                    }
                }
                Err(e) => {
                    anyhow::bail!("abyss reset: failed to remove {display}: {e}");
                }
            }
        }
        removed.push(display);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "removed": removed,
                "dry_run": dry_run,
                "scope": scope_label(scope),
            })
        );
    }
    Ok(())
}
