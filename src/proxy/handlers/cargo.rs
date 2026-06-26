//! Cargo command handlers: build, test, clippy.

use super::{ProxyContext, ProxyHandler};

// ---------------------------------------------------------------------------
// cargo build
// ---------------------------------------------------------------------------

pub struct CargoBuildHandler;

impl ProxyHandler for CargoBuildHandler {
    fn name(&self) -> &'static str {
        "cargo-build"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "cargo" && args.first().map(|s| s.as_str()) == Some("build")
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = if stdout.is_empty() { stderr } else { stdout };
        let lines: Vec<&str> = combined.lines().collect();

        let mut compiled = 0u32;
        let mut warnings = 0u32;
        let mut errors = Vec::new();
        let mut warning_details = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("Compiling ") {
                compiled += 1;
            } else if trimmed.starts_with("warning:") || trimmed.starts_with("warning[") {
                warnings += 1;
                if warning_details.len() < 5 {
                    warning_details.push(trimmed.to_string());
                }
            } else if trimmed.starts_with("error") {
                errors.push(trimmed.to_string());
            }
        }

        let mut out = String::new();

        if exit_code == 0 {
            out.push_str(&format!("build ok: {compiled} crate(s) compiled"));
            if warnings > 0 {
                out.push_str(&format!(", {warnings} warning(s)"));
            }
            out.push('\n');
            for w in &warning_details {
                out.push_str(&format!("  {w}\n"));
            }
            if warnings > 5 {
                out.push_str(&format!("  ... and {} more\n", warnings - 5));
            }
        } else {
            out.push_str(&format!(
                "build FAILED ({} error(s), {warnings} warning(s))\n",
                errors.len()
            ));
            for e in &errors {
                out.push_str(&format!("  {e}\n"));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// cargo test
// ---------------------------------------------------------------------------

pub struct CargoTestHandler;

impl ProxyHandler for CargoTestHandler {
    fn name(&self) -> &'static str {
        "cargo-test"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "cargo" && args.first().map(|s| s.as_str()) == Some("test")
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = format!("{stderr}\n{stdout}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut ignored = 0u32;
        let mut failures: Vec<String> = Vec::new();
        let mut in_failure_block = false;

        for line in &lines {
            let trimmed = line.trim();

            if trimmed.starts_with("test result:") {
                // Parse: "test result: ok. 42 passed; 0 failed; 3 ignored"
                for part in trimmed.split(';') {
                    let part = part.trim();
                    if let Some(n) = extract_count(part, "passed") {
                        passed += n;
                    }
                    if let Some(n) = extract_count(part, "failed") {
                        failed += n;
                    }
                    if let Some(n) = extract_count(part, "ignored") {
                        ignored += n;
                    }
                }
                in_failure_block = false;
            } else if trimmed == "failures:" || trimmed == "---- failures ----" {
                in_failure_block = true;
            } else if in_failure_block && trimmed.starts_with("---- ") && trimmed.ends_with(" ----")
            {
                let name = trimmed
                    .trim_start_matches("---- ")
                    .trim_end_matches(" ----")
                    .trim_end_matches(" stdout");
                failures.push(name.to_string());
            }
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!(
            "test {status}: {passed} passed, {failed} failed, {ignored} ignored\n"
        ));

        if !failures.is_empty() {
            out.push_str("failures:\n");
            for f in &failures {
                out.push_str(&format!("  - {f}\n"));
            }
            // Include failure output (last 30 lines of stderr for context)
            let stderr_lines: Vec<&str> = stderr.lines().collect();
            if stderr_lines.len() > 30 {
                out.push_str("\n(last 30 lines of output)\n");
            }
            let start = stderr_lines.len().saturating_sub(30);
            for line in &stderr_lines[start..] {
                let trimmed = line.trim();
                if !trimmed.is_empty()
                    && !trimmed.starts_with("Compiling ")
                    && !trimmed.starts_with("Downloading ")
                {
                    out.push_str(trimmed);
                    out.push('\n');
                }
            }
        }

        out
    }
}

fn extract_count(part: &str, label: &str) -> Option<u32> {
    if !part.contains(label) {
        return None;
    }
    part.split_whitespace().find_map(|w| w.parse::<u32>().ok())
}

// ---------------------------------------------------------------------------
// cargo clippy
// ---------------------------------------------------------------------------

pub struct CargoClippyHandler;

impl ProxyHandler for CargoClippyHandler {
    fn name(&self) -> &'static str {
        "cargo-clippy"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "cargo" && args.first().map(|s| s.as_str()) == Some("clippy")
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = if stdout.is_empty() { stderr } else { stdout };
        let lines: Vec<&str> = combined.lines().collect();

        let mut warnings: Vec<String> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("warning:") || trimmed.starts_with("warning[") {
                // Deduplicate "N warnings generated" style lines
                if !trimmed.contains("generated") {
                    warnings.push(trimmed.to_string());
                }
            } else if trimmed.starts_with("error") {
                errors.push(trimmed.to_string());
            }
        }

        let mut out = String::new();
        if exit_code == 0 && errors.is_empty() {
            if warnings.is_empty() {
                out.push_str("clippy: clean\n");
            } else {
                out.push_str(&format!("clippy: {} warning(s)\n", warnings.len()));
                for w in warnings.iter().take(15) {
                    out.push_str(&format!("  {w}\n"));
                }
                if warnings.len() > 15 {
                    out.push_str(&format!("  ... and {} more\n", warnings.len() - 15));
                }
            }
        } else {
            out.push_str(&format!(
                "clippy FAILED: {} error(s), {} warning(s)\n",
                errors.len(),
                warnings.len()
            ));
            for e in &errors {
                out.push_str(&format!("  {e}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_build_ok() {
        let h = CargoBuildHandler;
        let stderr = "\
   Compiling code-abyss v0.5.25
   Compiling another-crate v1.0.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.32s";
        let out = h.filter("", stderr, 0, &[], None);
        assert!(out.contains("build ok: 2 crate(s)"));
    }

    #[test]
    fn cargo_test_summary() {
        let h = CargoTestHandler;
        let stdout = "\
running 15 tests
test resolver_tiers ... ok
test test_something ... ok
test result: ok. 15 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out";
        let out = h.filter(stdout, "", 0, &[], None);
        assert!(out.contains("15 passed"));
        assert!(out.contains("0 failed"));
    }

    #[test]
    fn extract_count_works() {
        assert_eq!(extract_count("42 passed", "passed"), Some(42));
        assert_eq!(extract_count("0 failed", "failed"), Some(0));
        assert_eq!(extract_count("no match here", "passed"), None);
    }
}
