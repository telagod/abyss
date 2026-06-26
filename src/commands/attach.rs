use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::storage::Repository;

pub fn cmd_attach(host: &str, local: bool, proxy: bool) -> Result<()> {
    use crate::attach;

    if host == "all" {
        let results = attach::install_all(local);
        println!("\nabyss attach all — per-host summary:");
        let mut any_failed = false;
        for r in &results {
            match &r.outcome {
                Ok((status, path)) => {
                    println!("  {:<9} {status:>16}  {path}", r.host);
                }
                Err(msg) => {
                    println!("  {:<9} {msg}", r.host);
                    if !msg.starts_with("skipped:") {
                        any_failed = true;
                    }
                }
            }
        }
        if proxy {
            println!();
            if let Err(e) = attach::claude::install_proxy(local) {
                eprintln!("  proxy hook (claude): {e}");
            }
        }
        if any_failed {
            anyhow::bail!("one or more hosts failed to install — see summary above");
        }
        return Ok(());
    }

    if attach::SUPPORTED_HOSTS.contains(&host) {
        attach::install_host(host, local)?;
        if proxy {
            match host {
                "claude" => attach::claude::install_proxy(local)?,
                "codex" => attach::codex::install_proxy(local)?,
                "gemini" => attach::gemini::install_proxy(local)?,
                _ => {}
            }
        }
        Ok(())
    } else {
        anyhow::bail!(
            "unknown host: {host}; supported: {} (or `all`)",
            attach::SUPPORTED_HOSTS.join(", ")
        );
    }
}

pub fn cmd_setup(config: Config, local: bool, json: bool) -> Result<()> {
    eprintln!("abyss setup: indexing workspace...");
    super::index::cmd_index(config.clone(), json, false, None, false, None)?;

    eprintln!("\nabyss setup: installing hooks...");
    cmd_attach("all", local, true)?;

    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let stats = serde_json::json!({
        "files": repo.file_count()?,
        "symbols": repo.symbol_count()?,
        "refs": repo.ref_count()?,
    });

    eprintln!("\n✓ abyss setup complete");
    eprintln!(
        "  indexed: {} files, {} symbols, {} refs",
        stats["files"], stats["symbols"], stats["refs"]
    );
    eprintln!("  hooks: pre-edit + post-edit + proxy rewrite");
    eprintln!("\n  All agent commands now route through `abyss proxy`.");
    eprintln!("  Run `abyss gain` after a session to see token savings.");
    Ok(())
}

/// Emit the abyss skill manifest as JSON on stdout. Always exits 0 on
/// success. Default is pretty-printed because humans usually want it;
/// `--compact` collapses to a single line for machine pipelines.
pub fn cmd_skill_manifest(compact: bool) -> Result<()> {
    let v = crate::manifest::build_manifest();
    let rendered = if compact {
        serde_json::to_string(&v)?
    } else {
        serde_json::to_string_pretty(&v)?
    };
    println!("{rendered}");
    Ok(())
}

/// `abyss ingest` dispatcher. v0.5.15 ships only the SCIP subverb in
/// dry-run-summary mode; other languages of ingest (LSIF, raw
/// rust-analyzer JSON, ...) can land as siblings under this same
/// command tree without re-arranging the CLI.
pub fn cmd_ingest(action: super::IngestCmd, json: bool) -> Result<()> {
    match action {
        super::IngestCmd::Scip {
            path,
            dry_run,
            print_summary,
        } => cmd_ingest_scip(&path, dry_run, print_summary, json),
    }
}

/// SCIP ingest prototype (v0.5.15). Parses the JSON output of
/// `scip print --json`, prints a summary, and refuses to mutate the
/// index DB until the L-1 tier wiring lands in a follow-up patch.
///
/// The signature is deliberately wider than today's minimum (we accept
/// both `--dry-run` and `--print-summary`) so the CLI shape stays
/// stable when the real ingest path comes online — scripts pinning the
/// `--dry-run --print-summary` invocation today keep working unchanged.
fn cmd_ingest_scip(path: &Path, dry_run: bool, print_summary: bool, json: bool) -> Result<()> {
    use crate::ingest::{IngestSummary, parse_scip_input};

    let index = parse_scip_input(path)?;
    let summary = IngestSummary::from_index(&index);

    // Without `--dry-run`, refuse rather than silently no-op so an
    // operator can't be misled into thinking the index was updated.
    // The future L-1 tier will lift this gate.
    if !dry_run {
        anyhow::bail!(
            "abyss ingest scip: full DB ingest is not implemented in v0.5.15 \
             (prototype). Pass `--dry-run --print-summary` to inspect the input; \
             tracking the L-1 tier write path for a follow-up patch."
        );
    }

    if json {
        println!("{}", serde_json::to_string(&summary)?);
        return Ok(());
    }

    if print_summary {
        eprintln!(
            "scip ingest (dry-run): {} documents, {} occurrences, {} ref-candidates, \
             {} definitions across {} language(s)",
            summary.documents,
            summary.occurrences,
            summary.ref_candidates,
            summary.definitions,
            summary.languages.len(),
        );
        if !summary.languages.is_empty() {
            eprintln!("  languages: {}", summary.languages.join(", "));
        }
        eprintln!(
            "  note: v0.5.15 prototype — refs are NOT written to the index. \
             The L-1 ingest tier lands in a follow-up patch.",
        );
    } else {
        eprintln!(
            "scip ingest (dry-run): pass --print-summary to see the counts \
             ({} occurrences parsed).",
            summary.occurrences,
        );
    }
    Ok(())
}
