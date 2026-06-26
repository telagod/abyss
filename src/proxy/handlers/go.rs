//! Go ecosystem handlers: go test, go build.

use super::{ProxyContext, ProxyHandler};

pub struct GoTestHandler;

impl ProxyHandler for GoTestHandler {
    fn name(&self) -> &'static str { "go-test" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "go" && args.first().map(|s| s.as_str()) == Some("test")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut skipped = 0u32;
        let mut pkg_results: Vec<String> = Vec::new();
        let mut failure_output: Vec<String> = Vec::new();
        let mut in_fail = false;

        for line in &lines {
            let trimmed = line.trim();

            // "ok  	package/name	0.042s"
            if trimmed.starts_with("ok ") {
                passed += 1;
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    pkg_results.push(format!("✓ {} ({})", parts[1], parts[2]));
                }
            }
            // "FAIL	package/name	0.042s"
            if trimmed.starts_with("FAIL") && !trimmed.starts_with("FAIL\t") {
                failed += 1;
            } else if trimmed.starts_with("FAIL\t") {
                failed += 1;
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    pkg_results.push(format!("✗ {} ({})", parts[1], parts[2]));
                }
            }
            // "--- SKIP:" lines
            if trimmed.starts_with("--- SKIP:") {
                skipped += 1;
            }

            // Capture failure context
            if trimmed.starts_with("--- FAIL:") {
                in_fail = true;
                failure_output.push(trimmed.to_string());
            } else if in_fail {
                if trimmed.starts_with("--- ") || trimmed.starts_with("=== ") {
                    in_fail = false;
                } else if failure_output.len() < 30 {
                    failure_output.push(trimmed.to_string());
                }
            }
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!(
            "go test {status}: {passed} passed, {failed} failed, {skipped} skipped\n"
        ));

        if pkg_results.len() <= 20 {
            for r in &pkg_results {
                out.push_str(&format!("  {r}\n"));
            }
        } else {
            for r in pkg_results.iter().take(10) {
                out.push_str(&format!("  {r}\n"));
            }
            out.push_str(&format!("  ... {} more packages\n", pkg_results.len() - 10));
        }

        if !failure_output.is_empty() {
            out.push_str("\nfailures:\n");
            for line in &failure_output {
                out.push_str(&format!("  {line}\n"));
            }
        }
        out
    }
}

pub struct GoBuildHandler;

impl ProxyHandler for GoBuildHandler {
    fn name(&self) -> &'static str { "go-build" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "go" && args.first().map(|s| s.as_str()) == Some("build")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if exit_code == 0 && lines.iter().all(|l| l.trim().is_empty()) {
            return "go build: ok\n".into();
        }

        let mut errors: Vec<&str> = Vec::new();
        for line in &lines {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                errors.push(trimmed);
            }
        }

        let mut out = String::new();
        if exit_code == 0 {
            out.push_str("go build: ok\n");
        } else {
            out.push_str(&format!("go build FAILED ({} errors)\n", errors.len()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_test_summary() {
        let h = GoTestHandler;
        let stdout = "\
ok  \tgithub.com/user/repo/pkg1\t0.042s
ok  \tgithub.com/user/repo/pkg2\t0.108s
ok  \tgithub.com/user/repo/pkg3\t1.203s";
        let out = h.filter(stdout, "", 0, &[String::from("test"), String::from("./...")], None);
        assert!(out.contains("3 passed"), "pass count: {out}");
        assert!(out.contains("0 failed"), "fail count: {out}");
        assert!(out.contains("✓"), "checkmark: {out}");
    }

    #[test]
    fn go_test_failure_details() {
        let h = GoTestHandler;
        let stdout = "\
--- FAIL: TestFoo (0.00s)
    foo_test.go:15: expected 42, got 0
    foo_test.go:16: assertion failed
--- PASS: TestBar (0.01s)
FAIL\tgithub.com/user/repo/pkg1\t0.042s
ok  \tgithub.com/user/repo/pkg2\t0.108s";
        let out = h.filter(stdout, "", 1, &[String::from("test")], None);
        assert!(out.contains("FAILED"), "status: {out}");
        assert!(out.contains("1 failed"), "fail count: {out}");
        assert!(out.contains("failures:"), "failure section: {out}");
        assert!(out.contains("expected 42"), "failure details: {out}");
    }

    #[test]
    fn go_build_ok_empty() {
        let h = GoBuildHandler;
        let out = h.filter("", "", 0, &[], None);
        assert_eq!(out, "go build: ok\n");
    }

    #[test]
    fn go_build_errors() {
        let h = GoBuildHandler;
        let stderr = "\
./main.go:15:2: undefined: Foo
./main.go:20:5: cannot use x (type int) as type string";
        let out = h.filter("", stderr, 1, &[], None);
        assert!(out.contains("FAILED"), "status: {out}");
        assert!(out.contains("2 errors"), "error count: {out}");
        assert!(out.contains("undefined: Foo"), "error detail: {out}");
    }

    #[test]
    fn go_test_skipped() {
        let h = GoTestHandler;
        let stdout = "\
--- SKIP: TestIntegration (0.00s)
    test.go:10: requires database
ok  \tgithub.com/user/repo/pkg1\t0.042s";
        let out = h.filter(stdout, "", 0, &[String::from("test")], None);
        assert!(out.contains("1 skipped"), "skip count: {out}");
    }

    #[test]
    fn matches_go_test_and_build() {
        let h = GoTestHandler;
        assert!(h.matches("go", &[String::from("test")]));
        assert!(!h.matches("go", &[String::from("build")]));
        let hb = GoBuildHandler;
        assert!(hb.matches("go", &[String::from("build")]));
        assert!(!hb.matches("go", &[String::from("test")]));
    }
}
