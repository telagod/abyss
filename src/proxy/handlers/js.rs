//! JavaScript/TypeScript ecosystem handlers: npm/pnpm/yarn test, vitest, jest.

use super::{ProxyContext, ProxyHandler};

pub struct NpmTestHandler;

impl ProxyHandler for NpmTestHandler {
    fn name(&self) -> &'static str { "npm-test" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        (program == "npm" || program == "pnpm" || program == "yarn" || program == "npx")
            && args.first().map(|s| s.as_str()) == Some("test")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        // Try to detect the test runner
        let is_jest = lines.iter().any(|l| l.contains("Tests:") || l.contains("Test Suites:"));
        let is_vitest = lines.iter().any(|l| l.contains("✓") && l.contains("ms)"));

        if is_jest {
            return filter_jest(&lines, exit_code);
        }
        if is_vitest {
            return filter_vitest(&lines, exit_code);
        }

        // Generic npm test: just cap output
        generic_test_filter(&lines, exit_code)
    }
}

fn filter_jest(lines: &[&str], exit_code: i32) -> String {
    let mut out = String::new();
    let status = if exit_code == 0 { "ok" } else { "FAILED" };

    // Extract summary lines
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Tests:")
            || trimmed.starts_with("Test Suites:")
            || trimmed.starts_with("Snapshots:")
            || trimmed.starts_with("Time:")
        {
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    if out.is_empty() {
        out.push_str(&format!("jest {status}\n"));
    }

    // Show failures
    if exit_code != 0 {
        let mut in_fail = false;
        let mut fail_lines = 0;
        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("● ") || trimmed.starts_with("FAIL ") {
                in_fail = true;
                fail_lines = 0;
                out.push_str(trimmed);
                out.push('\n');
            } else if in_fail {
                fail_lines += 1;
                if fail_lines <= 10 {
                    out.push_str(trimmed);
                    out.push('\n');
                }
                if fail_lines == 10 {
                    in_fail = false;
                }
            }
        }
    }
    out
}

fn filter_vitest(lines: &[&str], exit_code: i32) -> String {
    let mut out = String::new();
    let mut passed = 0u32;
    let mut failed = 0u32;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.contains("✓") {
            passed += 1;
        } else if trimmed.contains("×") || trimmed.contains("✗") {
            failed += 1;
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    // Summary lines
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("Test Files")
            || trimmed.starts_with("Tests ")
            || trimmed.starts_with("Duration")
        {
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    let status = if exit_code == 0 { "ok" } else { "FAILED" };
    if out.is_empty() {
        out.push_str(&format!("vitest {status}: {passed} passed, {failed} failed\n"));
    }
    out
}

fn generic_test_filter(lines: &[&str], exit_code: i32) -> String {
    let status = if exit_code == 0 { "ok" } else { "FAILED" };
    if lines.len() <= 40 {
        return lines.join("\n");
    }
    let mut out = format!("npm test {status}\n");
    // Keep last 30 lines (likely summary)
    let start = lines.len().saturating_sub(30);
    for line in &lines[start..] {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jest_summary_extracted() {
        let h = NpmTestHandler;
        let stdout = "\
PASS src/app.test.tsx
PASS src/utils.test.ts
Test Suites: 2 passed, 2 total
Tests:       15 passed, 15 total
Snapshots:   0 total
Time:        3.42s";
        let out = h.filter(stdout, "", 0, &[String::from("test")], None);
        assert!(out.contains("Test Suites:"), "jest summary: {out}");
        assert!(out.contains("15 passed"), "test count: {out}");
    }

    #[test]
    fn jest_failure_shows_details() {
        let h = NpmTestHandler;
        let stdout = "\
FAIL src/app.test.tsx
  ● renders correctly
    Expected: 42
    Received: 0

Tests:       1 failed, 2 passed, 3 total
Test Suites: 1 failed, 1 passed, 2 total
Time:        1.23s";
        let out = h.filter(stdout, "", 1, &[String::from("test")], None);
        assert!(out.contains("1 failed"), "failure count: {out}");
        assert!(out.contains("FAIL "), "shows FAIL marker: {out}");
    }

    #[test]
    fn vitest_summary_extracted() {
        let h = NpmTestHandler;
        let stdout = "\
 ✓ src/utils.test.ts (3 tests) 12ms
 ✓ src/app.test.tsx (5 tests) 45ms

Test Files  2 passed (2)
Tests  8 passed (8)
Duration  1.23s";
        let out = h.filter(stdout, "", 0, &[String::from("test")], None);
        assert!(out.contains("Test Files"), "vitest summary: {out}");
        assert!(out.contains("8 passed"), "test count: {out}");
    }

    #[test]
    fn generic_short_passthrough() {
        let h = NpmTestHandler;
        let stdout = "line1\nline2\nline3";
        let out = h.filter(stdout, "", 0, &[String::from("test")], None);
        assert!(out.contains("line1"));
        assert!(out.contains("line3"));
    }

    #[test]
    fn matches_npm_pnpm_yarn() {
        let h = NpmTestHandler;
        let test_arg = vec![String::from("test")];
        assert!(h.matches("npm", &test_arg));
        assert!(h.matches("pnpm", &test_arg));
        assert!(h.matches("yarn", &test_arg));
        assert!(h.matches("npx", &test_arg));
        assert!(!h.matches("npm", &[String::from("install")]));
    }
}
