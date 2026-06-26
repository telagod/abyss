//! Linter handlers: eslint, ruff, flake8, mypy, tsc.

use super::{ProxyContext, ProxyHandler};

pub struct EslintHandler;

impl ProxyHandler for EslintHandler {
    fn name(&self) -> &'static str { "eslint" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "eslint"
            || program == "npx" && args.first().map(|s| s.as_str()) == Some("eslint")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut errors = 0u32;
        let mut warnings = 0u32;
        let mut files_with_issues: Vec<String> = Vec::new();
        let mut current_file: Option<String> = None;
        let mut file_issues: Vec<String> = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // File path lines (no leading whitespace, ends with extension)
            if !line.starts_with(' ') && !line.starts_with('\t')
                && (trimmed.contains(".js") || trimmed.contains(".ts")
                    || trimmed.contains(".jsx") || trimmed.contains(".tsx")
                    || trimmed.contains(".vue"))
            {
                if let Some(ref file) = current_file {
                    files_with_issues.push(format!("{file} ({} issues)", file_issues.len()));
                }
                current_file = Some(trimmed.to_string());
                file_issues.clear();
                continue;
            }
            // Issue lines (indented, contain line:col)
            if (line.starts_with(' ') || line.starts_with('\t')) && trimmed.contains("error") {
                errors += 1;
                file_issues.push(trimmed.to_string());
            } else if (line.starts_with(' ') || line.starts_with('\t')) && trimmed.contains("warning") {
                warnings += 1;
                file_issues.push(trimmed.to_string());
            }
        }

        if let Some(ref file) = current_file {
            files_with_issues.push(format!("{file} ({} issues)", file_issues.len()));
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "clean" } else { "issues found" };
        out.push_str(&format!("eslint {status}: {errors} error(s), {warnings} warning(s)\n"));

        if files_with_issues.len() <= 15 {
            for f in &files_with_issues {
                out.push_str(&format!("  {f}\n"));
            }
        } else {
            for f in files_with_issues.iter().take(10) {
                out.push_str(&format!("  {f}\n"));
            }
            out.push_str(&format!("  ... {} more files\n", files_with_issues.len() - 10));
        }
        out
    }
}

pub struct RuffHandler;

impl ProxyHandler for RuffHandler {
    fn name(&self) -> &'static str { "ruff" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "ruff"
            && args.first().map(|s| s.as_str()).is_some_and(|a| a == "check" || a == "format")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 30 {
            return combined;
        }

        let mut issues_by_rule: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut total = 0u32;

        for line in &lines {
            let trimmed = line.trim();
            // ruff output: "path.py:10:5: E501 Line too long"
            if let Some((_loc, rest)) = trimmed.split_once(": ")
                && let Some((rule, _msg)) = rest.split_once(' ')
                && rule.len() <= 8
                && rule.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            {
                *issues_by_rule.entry(rule.to_string()).or_default() += 1;
                total += 1;
            }
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "clean" } else { "issues found" };
        out.push_str(&format!("ruff {status}: {total} issue(s)\n"));

        if !issues_by_rule.is_empty() {
            let mut rules: Vec<_> = issues_by_rule.iter().collect();
            rules.sort_by(|a, b| b.1.cmp(a.1));
            out.push_str("  by rule:\n");
            for (rule, count) in rules.iter().take(10) {
                out.push_str(&format!("    {rule}: {count}\n"));
            }
            if rules.len() > 10 {
                out.push_str(&format!("    ... {} more rules\n", rules.len() - 10));
            }
        }
        out
    }
}

pub struct TscHandler;

impl ProxyHandler for TscHandler {
    fn name(&self) -> &'static str { "tsc" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "tsc"
            || program == "npx" && args.first().map(|s| s.as_str()) == Some("tsc")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 30 {
            return combined;
        }

        let mut errors: Vec<String> = Vec::new();
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.contains("error TS") {
                errors.push(trimmed.to_string());
            }
        }

        let mut out = String::new();
        if exit_code == 0 {
            out.push_str("tsc: clean (no errors)\n");
        } else {
            out.push_str(&format!("tsc: {} error(s)\n", errors.len()));
            for e in errors.iter().take(20) {
                out.push_str(&format!("  {e}\n"));
            }
            if errors.len() > 20 {
                out.push_str(&format!("  ... {} more\n", errors.len() - 20));
            }
        }
        out
    }
}
