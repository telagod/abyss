use anyhow::Result;

use crate::config::Config;
use crate::storage::Repository;

pub fn cmd_search(config: Config, query: &str, limit: usize, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    #[cfg(feature = "semantic")]
    let embedder = if repo.has_vectors()? {
        crate::embedding::Embedder::load(&config.model).ok()
    } else {
        None
    };
    #[cfg(not(feature = "semantic"))]
    let embedder: Option<crate::embedding::Embedder> = None;
    let engine = crate::search::SearchEngine::new(&repo, embedder.as_ref());
    let results = engine.search(query, limit)?;

    if json {
        println!("{}", serde_json::to_string(&results)?);
    } else {
        if results.is_empty() {
            eprintln!("no results");
            return Ok(());
        }
        for (i, r) in results.iter().enumerate() {
            println!(
                "{}. {} (L{}-L{}) [{}] score={:.4}",
                i + 1,
                r.file_path,
                r.start_line + 1,
                r.end_line + 1,
                r.kind,
                r.score
            );
            let preview: Vec<&str> = r.content.lines().take(3).collect();
            for line in &preview {
                println!("   {line}");
            }
            if r.content.lines().count() > 3 {
                println!("   ...");
            }
            println!();
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_callers(
    config: Config,
    symbol: &str,
    limit: usize,
    min_confidence: f64,
    include_tests: bool,
    calls_only: bool,
    types_only: bool,
    inherits_only: bool,
    _all_deps: bool,
    json: bool,
) -> Result<()> {
    use crate::graph::CallerKindFilter;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = crate::graph::GraphQuery::new(&repo);
    // `--all-deps` is a self-documenting alias for the default — it
    // collapses to the same `Both` branch as "no flag at all". Kept as a
    // dedicated arg so scripts can spell intent and so clap's
    // `conflicts_with_all` machinery enforces mutual exclusion with the
    // restricting flags.
    let kind_filter = match (calls_only, types_only, inherits_only) {
        (true, false, false) => CallerKindFilter::CallsOnly,
        (false, true, false) => CallerKindFilter::TypesOnly,
        (false, false, true) => CallerKindFilter::InheritsOnly,
        _ => CallerKindFilter::Both,
    };
    let restricted = calls_only || types_only || inherits_only;
    // `--limit 0` → unlimited (capped internally at UNLIMITED_CAP so a hot
    // framework primitive — Django Model, hono Context — can't OOM the
    // process). Two orders of magnitude above any caller list seen in
    // dogfood (Django Model topped at ~1k).
    const UNLIMITED_CAP: usize = 50_000;
    let effective_limit = if limit == 0 { UNLIMITED_CAP } else { limit };
    let result = gq.find_callers_filtered_kinds(
        symbol,
        effective_limit,
        min_confidence,
        include_tests,
        kind_filter,
    )?;
    // Total visible count — for the "showing N of M" footer (B3). Counted
    // server-side under the same filter so M honestly matches the view; test
    // callers are not counted toward M when include_tests=false (they're
    // invisible to the agent here).
    let total_available = repo.count_callers_at(
        symbol,
        kind_filter.as_slice(),
        min_confidence,
        include_tests,
    )?;
    let shown = result.callers.len();
    let was_capped = shown < total_available;

    if json {
        // Augment JSON with the total + cap info so MCP / scripts can
        // reproduce the footer without re-querying.
        let payload = serde_json::json!({
            "callers": result.callers,
            "excluded_tests": result.excluded_tests,
            "total_available": total_available,
            "limit": limit,
            "was_capped": was_capped,
        });
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        if result.callers.is_empty() && result.excluded_tests == 0 {
            eprintln!("no callers found for '{symbol}'");
            return Ok(());
        }
        // Header surfaces the test-exclusion contract so an agent that sees an
        // empty or short list knows whether to retry with --include-tests.
        let header = if include_tests {
            format!("callers of '{symbol}' ({} found):\n", result.callers.len())
        } else if result.excluded_tests > 0 {
            format!(
                "callers of '{symbol}' ({} prod, {} tests excluded — use --include-tests to see all):\n",
                result.callers.len(),
                result.excluded_tests
            )
        } else {
            format!("callers of '{symbol}' ({} prod):\n", result.callers.len())
        };
        println!("{header}");
        // When the user did not restrict via a flag, the list may mix call,
        // type_ref, and inherit edges. Suffix each row with the edge kind so
        // the agent can tell "X() invokes this" from "X uses this in a type
        // position" from "X inherits from this" without re-querying. When
        // restricted, the kind is implicit — keep the legacy compact format.
        for (i, c) in result.callers.iter().enumerate() {
            let t = if c.is_test { " [test]" } else { "" };
            let kind_suffix = if restricted {
                String::new()
            } else {
                format!(", {}", short_kind(&c.kind))
            };
            println!(
                "  {}. {}:{} → {}(){t}  ({:.0}%{kind_suffix})",
                i + 1,
                c.file_path,
                c.line + 1,
                c.symbol,
                c.confidence * 100.0,
            );
        }
        // Capped-list footer (B3). hono dogfood (2026-06-17): `callers
        // Context` showed 20 but 235 refs existed — no footer meant the
        // agent stopped reading at 20 and missed the real call sites.
        if was_capped {
            if limit == 0 {
                println!(
                    "\n(showing {shown} of {total_available} total — hit the {UNLIMITED_CAP} safety cap)"
                );
            } else {
                println!(
                    "\n(showing {shown} of {total_available} total — use --limit 0 for all, --limit N for more)"
                );
            }
        }
    }
    Ok(())
}

/// Short edge-kind label for the mixed callers listing: `call`, `field`,
/// `type`. Unknown kinds round-trip verbatim so a future edge type doesn't
/// silently masquerade as `call`.
fn short_kind(kind: &str) -> &str {
    match kind {
        "call" => "call",
        "field_access" => "field",
        "type_ref" => "type",
        other => other,
    }
}

pub fn cmd_impact(
    config: Config,
    symbol: &str,
    depth: u32,
    min_confidence: f64,
    calls_only: bool,
    json: bool,
) -> Result<()> {
    use crate::graph::CallerKindFilter;
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let gq = crate::graph::GraphQuery::new(&repo);
    // Default to the agent-facing superset (matches `abyss callers`).
    // `--calls-only` reverts to the v0.5.1 legacy "who invokes this function"
    // behaviour for users who want the function-only blast radius.
    let kind_filter = if calls_only {
        CallerKindFilter::CallsOnly
    } else {
        CallerKindFilter::Both
    };
    let result = gq.impact_analysis_filtered(symbol, depth, min_confidence, kind_filter)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "impact: {}  direct={}  transitive={}  tests={}  uncovered={}  risk={:.1}/10",
            result.target,
            result.direct_callers.len(),
            result.transitive_callers.len(),
            result.affected_tests.len(),
            result.uncovered_paths.len(),
            result.risk_score
        );
        for f in &result.risk_factors {
            println!("  ⚠ {f}");
        }
        for c in result.direct_callers.iter().take(10) {
            let t = if c.is_test { " [test]" } else { "" };
            println!("  {}:{} → {}(){t}", c.file_path, c.line + 1, c.symbol);
        }
    }
    Ok(())
}

pub fn cmd_history(config: Config, file: &str, symbol: Option<&str>, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let result =
        crate::temporal::evolution::trace_evolution(&config.workspace, &repo, file, symbol)?;

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "evolution: {}  changes={}  authors={}  churn={:.1}x",
            result.file_path, result.total_changes, result.unique_authors, result.churn_rate
        );
        for c in result.commits.iter().take(10) {
            println!("  {} {} {:<16} {}", c.hash, c.date, c.author, c.message);
        }
        for c in result.coupled_files.iter().take(5) {
            println!(
                "  ↔ {} ({}×, {:.0}%)",
                c.path,
                c.co_changes,
                c.coupling_score * 100.0
            );
        }
    }
    Ok(())
}

pub fn cmd_context(config: Config, file: &str, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let Some(output) = crate::context::build_file_context(&repo, file)? else {
        eprintln!("file not found: {file}");
        return Ok(());
    };
    let file_path = output["file"].as_str().unwrap_or(file).to_string();
    let sym_callers = output["symbols_with_external_callers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let deps = output["dependencies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let hotspot = output.get("hotspot").filter(|h| !h.is_null()).cloned();
    let coupled = output["coupled_files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let symbols_defined = output["symbols_defined"].as_u64().unwrap_or(0);

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("=== {} ===\n", file_path);
        println!(
            "{} symbols defined, {} with external callers\n",
            symbols_defined,
            sym_callers.len()
        );

        for sc in &sym_callers {
            let sym = sc["symbol"].as_str().unwrap_or("");
            let callers = sc["external_callers"].as_array().unwrap();
            println!("  {}() ← {} callers", sym, callers.len());
            // Production callers are the pre-edit safety contract: list ALL of
            // them — silent truncation once made an agent miss a call site.
            // Test callers are capped, with the remainder counted explicitly.
            let (prod, test): (Vec<_>, Vec<_>) = callers
                .iter()
                .partition(|c| !c["is_test"].as_bool().unwrap_or(false));
            for c in &prod {
                println!(
                    "    {}:{} → {}()",
                    c["file"].as_str().unwrap_or(""),
                    c["line"],
                    c["caller"].as_str().unwrap_or("")
                );
            }
            for c in test.iter().take(3) {
                println!(
                    "    {}:{} → {}() [test]",
                    c["file"].as_str().unwrap_or(""),
                    c["line"],
                    c["caller"].as_str().unwrap_or("")
                );
            }
            if test.len() > 3 {
                println!("    … and {} more test callers", test.len() - 3);
            }
        }

        if !deps.is_empty() {
            println!("\n  depends on:");
            for d in &deps {
                println!(
                    "    → {} ({})",
                    d["name"].as_str().unwrap_or(""),
                    d["file"].as_str().unwrap_or("")
                );
            }
        }

        if let Some(h) = &hotspot {
            println!(
                "\n  hotspot: score={:.0}  changes={}  cc={:.0}",
                h["score"].as_f64().unwrap_or(0.0),
                h["changes_30d"].as_i64().unwrap_or(0),
                h["complexity"].as_f64().unwrap_or(0.0)
            );
        }

        if !coupled.is_empty() {
            println!("\n  coupled files:");
            for c in &coupled {
                println!(
                    "    ↔ {} ({}×, {})",
                    c["file"].as_str().unwrap_or(""),
                    c["co_changes"],
                    c["coupling"].as_str().unwrap_or("")
                );
            }
        }
    }
    Ok(())
}

pub fn cmd_where(config: Config, file: &str, json: bool) -> Result<()> {
    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let Some(view) = crate::context::where_summary(&repo, file)? else {
        if json {
            println!("{}", serde_json::json!({"file": file, "found": false}));
        } else {
            eprintln!("not indexed: {file}");
        }
        return Ok(());
    };

    if json {
        println!("{}", serde_json::to_string(&view)?);
        return Ok(());
    }

    let layer = view["layer"].as_str().unwrap_or("unknown");
    let role = view["role"].as_str().unwrap_or("unknown");
    let module = view["module_label"].as_str().unwrap_or("—");
    let conf = view["layer_conf"].as_f64().unwrap_or(0.0);
    let centrality = view["centrality"].as_f64().unwrap_or(0.0);
    let in_deg = view["in_degree"].as_u64().unwrap_or(0);
    let out_deg = view["out_degree"].as_u64().unwrap_or(0);
    let depth = view["depth_from_entry"]
        .as_u64()
        .map(|d| d.to_string())
        .unwrap_or_else(|| String::from("—"));
    let path = view["file"].as_str().unwrap_or(file);

    println!("where: {path}");
    println!("  layer={layer}  module={module}  role={role}  conf={conf:.2}");
    println!("  depth_from_entry={depth}  centrality={centrality:.2}  in={in_deg} out={out_deg}");

    // Compact one-line signal trace — useful for "why is this file classified
    // as X?" without dumping the full JSON.
    let signals = &view["signals"];
    if !signals.is_null() {
        let dir_hints = signals["dir"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|h| {
                        format!(
                            "{}×{:.1}",
                            h["layer"].as_str().unwrap_or("?"),
                            h["weight"].as_f64().unwrap_or(0.0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let name_hints = signals["name"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|h| {
                        format!(
                            "{}×{:.1}",
                            h["layer"].as_str().unwrap_or("?"),
                            h["weight"].as_f64().unwrap_or(0.0)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let entry = signals["entry"].as_bool().unwrap_or(false);
        println!("  signals: dir=[{dir_hints}], name=[{name_hints}], entry={entry}");
    }
    if let Some(note) = view.get("note").and_then(|v| v.as_str()) {
        println!("  note: {note}");
    }
    Ok(())
}

pub fn cmd_completion(shell: clap_complete::Shell, cmd: &mut clap::Command) -> Result<()> {
    clap_complete::generate(shell, cmd, "abyss", &mut std::io::stdout());
    Ok(())
}
