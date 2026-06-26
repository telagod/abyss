use anyhow::Result;

use crate::config::Config;
use crate::storage::Repository;

pub fn cmd_proxy(
    config: Config,
    command: Vec<String>,
    force_tee: bool,
    explain: bool,
    json: bool,
) -> Result<()> {
    use crate::proxy::{self, estimate_tokens, never_worse};

    if command.is_empty() {
        anyhow::bail!("abyss proxy: no command given");
    }

    let program = &command[0];
    let args: Vec<String> = command[1..].to_vec();
    let full_cmd = command.join(" ");

    let start = std::time::Instant::now();

    let result = proxy::runner::run_captured(program, &args)?;
    let exec_ms = start.elapsed().as_millis() as u64;

    // Zero-copy raw output: borrows when only one stream has data (>90%
    // of commands), allocates only when both stdout and stderr are non-empty.
    let raw = result.raw_output();
    let raw_output: &str = &raw;

    if result.truncated {
        let tee_dir = config.workspace.join(".code-abyss").join("tee");
        if let Ok(Some(path)) = proxy::tee::write_tee(&tee_dir, &full_cmd, raw_output) {
            eprintln!(
                "[abyss proxy] output exceeded 10MiB, truncated. Full: {}",
                path.display()
            );
        }
    }

    // Route: Rust handler → TOML filter → passthrough
    let handlers = proxy::handlers::all_handlers();
    let mut filter_reason = "passthrough (no matching handler or filter)";

    let filtered = if let Some(handler) = proxy::handlers::find_handler(&handlers, program, &args) {
        filter_reason = handler.name();
        let files_in_output = extract_file_paths_from_output(&result.stdout);
        let file_refs: Vec<&str> = files_in_output.iter().map(|s| s.as_str()).collect();
        let ctx = proxy::ProxyContext::from_index(&config, &file_refs);
        handler.filter(
            &result.stdout,
            &result.stderr,
            result.exit_code,
            &args,
            ctx.as_ref(),
        )
    } else {
        let builtin = proxy::filter::builtin_filters();
        let project_filters =
            proxy::filter::load_filters(&config.workspace.join(".code-abyss").join("filters.toml"));
        let mut all_filters = builtin;
        all_filters.extend(project_filters);

        if let Some(def) = proxy::filter::find_filter(&all_filters, &full_cmd) {
            filter_reason = "toml-filter";
            proxy::filter::apply_filter(def, raw_output)
        } else {
            let line_count = raw_output.lines().count();
            if line_count > 100 {
                filter_reason = "passthrough (line-capped at 100)";
                let lines: Vec<&str> = raw_output.lines().collect();
                let head = 60;
                let tail = 30;
                let mut out: String = lines[..head].join("\n");
                out.push_str(&format!(
                    "\n... ({} lines skipped)\n",
                    line_count - head - tail
                ));
                out.push_str(&lines[line_count - tail..].join("\n"));
                out
            } else {
                raw_output.to_string()
            }
        }
    };

    // Never-worse guard
    let output = never_worse(raw_output, &filtered);

    // Tee: save full output if needed
    let tee_mode = if force_tee {
        proxy::tee::TeeMode::Always
    } else {
        proxy::tee::TeeMode::Failures
    };
    if proxy::tee::should_tee(tee_mode, result.exit_code, raw_output.len()) {
        let tee_dir = config.workspace.join(".code-abyss").join("tee");
        if let Ok(Some(path)) = proxy::tee::write_tee(&tee_dir, &full_cmd, raw_output) {
            eprintln!("{}", proxy::tee::tee_hint(&path));
        }
    }

    // Track token savings (best-effort)
    let is_first_proxy = if config.db_path.exists()
        && let Ok(repo) = Repository::open(&config.db_path, config.model.dimensions)
    {
        let conn = repo.conn();
        let first = !proxy::tracking::has_any_data(conn);
        let _ = proxy::tracking::ensure_table(conn);
        let _ = proxy::tracking::record(
            conn,
            &full_cmd,
            raw_output,
            output,
            exec_ms,
            config.workspace.file_name().and_then(|n| n.to_str()),
        );
        first
    } else {
        false
    };

    if json {
        let raw_tokens = estimate_tokens(raw_output);
        let filtered_tokens = estimate_tokens(output);
        println!(
            "{}",
            serde_json::json!({
                "command": full_cmd,
                "exit_code": result.exit_code,
                "raw_tokens": raw_tokens,
                "filtered_tokens": filtered_tokens,
                "savings_pct": if raw_tokens > 0 {
                    ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64) * 100.0
                } else { 0.0 },
                "output": output,
            })
        );
    } else {
        print!("{output}");
        if is_first_proxy {
            eprintln!("\n[abyss proxy] tracking token savings → run `abyss gain` to see report");
        }
    }

    if explain {
        let raw_tokens = estimate_tokens(raw_output);
        let filtered_tokens = estimate_tokens(output);
        let saved = raw_tokens.saturating_sub(filtered_tokens);
        let pct = if raw_tokens > 0 {
            (saved as f64 / raw_tokens as f64) * 100.0
        } else {
            0.0
        };
        eprintln!("\n[explain] handler: {filter_reason}");
        eprintln!(
            "[explain] raw: {raw_tokens} tokens → filtered: {filtered_tokens} tokens ({pct:.0}% saved)"
        );
        eprintln!(
            "[explain] exec: {exec_ms}ms | truncated: {}",
            result.truncated
        );
        let explain_files = extract_file_paths_from_output(&result.stdout);
        let explain_refs: Vec<&str> = explain_files.iter().map(|s| s.as_str()).collect();
        if let Some(ctx) = proxy::ProxyContext::from_index(&config, &explain_refs)
            && !ctx.impacted_callers.is_empty()
        {
            eprintln!(
                "[explain] semantic: {} files with blast-radius data",
                ctx.impacted_callers.len()
            );
        }
    }

    // Propagate exit code
    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }
    Ok(())
}

pub fn extract_file_paths_from_output(output: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        // git diff: "diff --git a/path b/path"
        if trimmed.starts_with("diff --git")
            && let Some(path) = trimmed.split(" b/").nth(1)
        {
            files.push(path.to_string());
        }
        // git status: "modified: path" / "new file: path"
        if let Some(rest) = trimmed.strip_prefix("modified:") {
            files.push(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("new file:") {
            files.push(rest.trim().to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("deleted:") {
            files.push(rest.trim().to_string());
        }
    }
    files
}

pub fn cmd_gain(config: Config, days: u32, json: bool) -> Result<()> {
    use crate::proxy::tracking;

    if !config.db_path.exists() {
        anyhow::bail!(
            "no index found at {} — run `abyss index` first",
            config.db_path.display()
        );
    }

    let repo = Repository::open(&config.db_path, config.model.dimensions)?;
    let conn = repo.conn();

    let summary = tracking::gain_summary(conn, days)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "total_commands": summary.total_commands,
                "total_raw_tokens": summary.total_raw_tokens,
                "total_filtered_tokens": summary.total_filtered_tokens,
                "total_saved_tokens": summary.total_saved_tokens,
                "avg_savings_pct": summary.avg_savings_pct,
                "top_commands": summary.top_commands.iter()
                    .map(|(cmd, saved, pct)| serde_json::json!({
                        "command": cmd, "saved_tokens": saved, "savings_pct": pct
                    }))
                    .collect::<Vec<_>>(),
            })
        );
    } else {
        print!("{}", tracking::render_gain(&summary));
    }
    Ok(())
}

pub fn cmd_rewrite(command: Vec<String>) -> Result<()> {
    use crate::proxy::rewrite;

    let cmd_str = command.join(" ");
    match rewrite::rewrite_command(&cmd_str) {
        Some(rewritten) => {
            println!("{rewritten}");
            Ok(())
        }
        None => {
            // Exit code 1 = no rewrite available
            std::process::exit(1);
        }
    }
}
