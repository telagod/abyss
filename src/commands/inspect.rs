use anyhow::Result;

use crate::config::Config;
use crate::storage::Repository;

/// Best-effort daemon liveness summary used by `config show`. Returns a
/// `serde_json::Value` so callers can inline it into the payload without
/// caring about whether the pidfile / `kill(pid,0)` succeeded.
pub fn daemon_state_view(config: &Config) -> serde_json::Value {
    #[cfg(unix)]
    {
        use crate::daemon::DaemonPaths;
        let paths = DaemonPaths::from_config(config);
        if !paths.pid.exists() {
            return serde_json::json!({"running": false});
        }
        let pid_raw = std::fs::read_to_string(&paths.pid).ok();
        let pid: Option<u32> = pid_raw
            .as_deref()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let alive = match pid {
            // SAFETY: `kill(pid, 0)` is the standard liveness probe — no
            // signal sent, just returns 0 when the pid is reachable from
            // this process. Identical to the `daemon status` probe.
            Some(p) => {
                let rc = unsafe { libc::kill(p as libc::pid_t, 0) };
                rc == 0
            }
            None => false,
        };
        let uptime_secs = paths
            .pid
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs());
        serde_json::json!({
            "running": alive,
            "pid": pid,
            "uptime_secs": uptime_secs,
            "pidfile": paths.pid.display().to_string(),
            "socket": paths.socket.display().to_string(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        serde_json::json!({"running": false, "note": "daemon is Unix-only"})
    }
}

/// `abyss config show`: render the effective config + observable state so
/// operators and agents can sanity-check what abyss is actually running
/// without spelunking through `.code-abyss/`. Read-only by contract —
/// never opens the DB for writes, never mutates `arch.toml`, never talks
/// to the daemon's socket beyond a liveness probe.
///
/// Output shape:
///   * workspace / db / arch_toml_path / log_path
///   * schema_version (from `meta` table; 0 if no index yet)
///   * arch_toml: present + layer_rules + ignore_rules (when present)
///   * dictionary: built-in rule count
///   * index counts: files / chunks / symbols / refs (when DB exists)
///   * arch_layer_counts: per-layer file count from `arch_facts`
///   * daemon: { running, pid, uptime_secs } (best-effort)
pub fn cmd_config_show(config: Config, json: bool) -> Result<()> {
    let workspace = config.workspace.display().to_string();
    let db_path = config.db_path.display().to_string();
    let arch_toml_path = config
        .workspace
        .join(".code-abyss")
        .join("arch.toml")
        .display()
        .to_string();
    let log_path = config
        .workspace
        .join(".code-abyss")
        .join("daemon.log")
        .display()
        .to_string();
    let dictionary_builtin_rules = crate::arch::builtin_rule_count();

    // arch.toml — best-effort. We use the existing loader so we surface the
    // exact same view the indexer sees (malformed TOML → "not loaded").
    let arch_toml = crate::arch::load_overrides(&config.workspace);
    let arch_toml_view = match &arch_toml {
        Some(o) => serde_json::json!({
            "present": true,
            "path": arch_toml_path,
            "layer_rules": o.layer_rule_count(),
            "ignore_rules": o.ignore_rule_count(),
        }),
        None => serde_json::json!({
            "present": false,
            "path": arch_toml_path,
        }),
    };

    // Index facts — opening the repo is the only "expensive" step here.
    // Missing DB is a normal state for a fresh workspace; we surface it
    // explicitly rather than failing.
    let (schema_version, files, chunks, symbols, refs, arch_layer_counts) =
        if config.db_path.exists() {
            match Repository::open(&config.db_path, config.model.dimensions) {
                Ok(repo) => {
                    let conn = repo.conn();
                    let schema: i64 = conn
                        .query_row(
                            "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    let files = repo.file_count().unwrap_or(0);
                    let chunks = repo.chunk_count().unwrap_or(0);
                    let symbols = repo.symbol_count().unwrap_or(0);
                    let refs = repo.ref_count().unwrap_or(0);

                    let mut by_layer: Vec<(String, i64)> = Vec::new();
                    let layer_query = conn.prepare(
                        "SELECT layer, COUNT(*) FROM arch_facts GROUP BY layer ORDER BY layer",
                    );
                    if let Ok(mut stmt) = layer_query
                        && let Ok(rows) = stmt
                            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    {
                        for row in rows.flatten() {
                            by_layer.push(row);
                        }
                    }
                    (schema, files, chunks, symbols, refs, by_layer)
                }
                Err(_) => (0, 0, 0, 0, 0, Vec::new()),
            }
        } else {
            (0, 0, 0, 0, 0, Vec::new())
        };
    let index_present = config.db_path.exists();
    let arch_layer_counts_json: serde_json::Value = arch_layer_counts
        .iter()
        .map(|(layer, count)| serde_json::json!({"layer": layer, "files": count}))
        .collect();

    // Daemon liveness — read pidfile + `kill(pid, 0)` on unix, omit on
    // platforms where the daemon doesn't exist. Best-effort: a stale
    // pidfile reports `running: false` so agents don't act on ghosts.
    let daemon_view = daemon_state_view(&config);

    let payload = serde_json::json!({
        "workspace": workspace,
        "db_path": db_path,
        "log_path": log_path,
        "schema_version": schema_version,
        "index_present": index_present,
        "arch_toml": arch_toml_view,
        "dictionary": {
            "builtin_rules": dictionary_builtin_rules,
        },
        "counts": {
            "files": files,
            "chunks": chunks,
            "symbols": symbols,
            "refs": refs,
        },
        "arch_layer_counts": arch_layer_counts_json,
        "daemon": daemon_view,
    });

    if json {
        println!("{}", serde_json::to_string(&payload)?);
        return Ok(());
    }

    // TOML output — `toml::to_string_pretty` accepts any Serialize value
    // and renders tables/arrays in the multi-line form humans expect. The
    // single-line `toml::Value::Display` would jam the entire config onto
    // one row, defeating the point of inspecting it.
    match toml::to_string_pretty(&payload) {
        Ok(s) => {
            println!("# abyss config show");
            print!("{s}");
        }
        Err(_) => {
            // Defensive fallback — JSON is always valid output.
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

pub fn cmd_stats(config: Config, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let s = serde_json::json!({
        "files": repo.file_count()?,
        "chunks": repo.chunk_count()?,
        "symbols": repo.symbol_count()?,
        "refs": repo.ref_count()?,
        "workspace": config.workspace.display().to_string(),
    });

    if json {
        println!("{}", serde_json::to_string(&s)?);
    } else {
        println!(
            "abyss: {} files, {} chunks, {} symbols, {} refs",
            s["files"], s["chunks"], s["symbols"], s["refs"]
        );
    }
    Ok(())
}

/// Minimum git history we need before the hotspot ranking is informative.
/// Below this the change_count_30d signal is mostly zero (shallow clone or
/// fresh repo) and the empty list reads as "no risk" instead of "no data".
pub const MAP_MIN_COMMITS_FOR_HOTSPOTS: i64 = 10;

/// Hint to print under an empty Hotspots heading. Pure function so tests can
/// pin the string without spinning up a binary. Returns `None` when the
/// hotspot list is non-empty (the heading already has content).
///
/// Two failure modes deserve distinct hints:
/// * shallow / fresh repo (`total_commits < MAP_MIN_COMMITS_FOR_HOTSPOTS`)
///   → "insufficient history" so the agent knows to try a deeper clone
/// * historical repo with no recent activity → "no files changed in the
///   last 30 days" so the agent doesn't keep retrying / blames the index
pub fn empty_hotspots_hint(is_empty: bool, total_commits: i64) -> Option<String> {
    if !is_empty {
        return None;
    }
    Some(if total_commits < MAP_MIN_COMMITS_FOR_HOTSPOTS {
        format!(
            "  (insufficient history: {total_commits} commits available; need ≥{MAP_MIN_COMMITS_FOR_HOTSPOTS} — try a deeper clone)"
        )
    } else {
        String::from(
            "  (no files changed in the last 30 days — try `abyss history <path>` for older activity)",
        )
    })
}

pub fn cmd_map(config: Config, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let hotspots = crate::temporal::hotspot::top_hotspots(&repo, limit)?;
    let coupled = crate::temporal::coupling::top_coupled(&repo, limit)?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "hotspots": hotspots, "coupling": coupled
            }))?
        );
    } else {
        println!("═══ Hotspots ═══");
        for (i, h) in hotspots.iter().enumerate() {
            println!(
                "  {:2}. {:<55} score={:.0}  Δ{}  cc={:.0}  👤{}",
                i + 1,
                h.file_path,
                h.hotspot_score,
                h.change_count,
                h.complexity,
                h.unique_authors
            );
        }
        if hotspots.is_empty() {
            // Distinguishing "no risk" from "no data" matters: on a shallow
            // clone the empty hotspot list reads as a green light. Mirror
            // the coupling suppression message style so the two failure
            // modes look related.
            let total_commits: i64 = repo
                .conn()
                .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
                .unwrap_or(0);
            if let Some(hint) = empty_hotspots_hint(true, total_commits) {
                println!("{hint}");
            }
        }
        if !coupled.is_empty() {
            println!("\n═══ Coupling ═══");
            for (i, c) in coupled.iter().take(10).enumerate() {
                println!(
                    "  {:2}. {} ↔ {}  ({}×, {:.0}%)",
                    i + 1,
                    c.file_a,
                    c.file_b,
                    c.co_changes,
                    c.coupling_score * 100.0
                );
            }
        }
    }
    Ok(())
}
