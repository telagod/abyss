//! Python ecosystem handlers: pytest, unittest.

use super::{ProxyContext, ProxyHandler};

pub struct PytestHandler;

impl ProxyHandler for PytestHandler {
    fn name(&self) -> &'static str { "pytest" }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "pytest" || program == "python" || program == "python3"
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, args: &[String], _ctx: Option<&ProxyContext>) -> String {
        // Only intercept when running tests
        let is_test = args.iter().any(|a| a == "-m" || a == "pytest" || a.contains("test"));
        if !is_test && self.matches_only_pytest(args) {
            // Fall through for non-test python invocations
        }

        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut out = String::new();
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut errors = 0u32;
        let mut skipped = 0u32;
        let mut failure_sections: Vec<String> = Vec::new();
        let mut in_failure = false;
        let mut fail_buf = String::new();
        let mut fail_lines = 0u32;

        for line in &lines {
            let trimmed = line.trim();

            // pytest short summary
            if trimmed.starts_with("=") && trimmed.contains("passed") {
                // "= 42 passed, 3 failed in 1.23s ="
                for word in trimmed.split_whitespace() {
                    if let Ok(n) = word.parse::<u32>() {
                        let next_words: Vec<&str> = trimmed.split_whitespace().collect();
                        let idx = next_words
                            .iter()
                            .position(|&w| w == word);
                        if let Some(i) = idx && let Some(&label) = next_words.get(i + 1) {
                            match label.trim_end_matches(',') {
                                "passed" => passed = n,
                                "failed" => failed = n,
                                "error" | "errors" => errors = n,
                                "skipped" | "deselected" => skipped = n,
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Capture failure details
            if trimmed.starts_with("FAILED ") || trimmed.starts_with("___") {
                if in_failure && !fail_buf.is_empty() {
                    failure_sections.push(fail_buf.clone());
                }
                in_failure = true;
                fail_buf.clear();
                fail_lines = 0;
            }
            if in_failure {
                fail_lines += 1;
                if fail_lines <= 15 {
                    fail_buf.push_str(trimmed);
                    fail_buf.push('\n');
                }
            }
        }

        if in_failure && !fail_buf.is_empty() {
            failure_sections.push(fail_buf);
        }

        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!(
            "pytest {status}: {passed} passed, {failed} failed, {errors} errors, {skipped} skipped\n"
        ));

        for section in failure_sections.iter().take(5) {
            out.push_str(section);
            out.push('\n');
        }
        if failure_sections.len() > 5 {
            out.push_str(&format!(
                "... {} more failure(s)\n",
                failure_sections.len() - 5
            ));
        }

        out
    }
}

impl PytestHandler {
    fn matches_only_pytest(&self, args: &[String]) -> bool {
        args.iter().any(|a| a == "pytest" || a == "-m")
    }
}
