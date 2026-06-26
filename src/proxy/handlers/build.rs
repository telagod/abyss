//! Build system handlers: make, maven (mvn), gradle, pip install.

use super::{ProxyContext, ProxyHandler};

pub struct MakeHandler;

impl ProxyHandler for MakeHandler {
    fn name(&self) -> &'static str { "make" }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "make" || program == "gmake"
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 30 {
            return combined;
        }

        let mut errors: Vec<&str> = Vec::new();
        let mut warnings = 0u32;

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.contains(": error:") || trimmed.starts_with("make:") && trimmed.contains("Error") {
                errors.push(trimmed);
            } else if trimmed.contains(": warning:") {
                warnings += 1;
            }
        }

        let mut out = String::new();
        if exit_code == 0 {
            out.push_str(&format!("make ok ({} warnings)\n", warnings));
        } else {
            out.push_str(&format!("make FAILED: {} error(s), {} warning(s)\n", errors.len(), warnings));
            for e in errors.iter().take(15) {
                out.push_str(&format!("  {e}\n"));
            }
            if errors.len() > 15 {
                out.push_str(&format!("  ... {} more\n", errors.len() - 15));
            }
        }
        out
    }
}

pub struct MvnHandler;

impl ProxyHandler for MvnHandler {
    fn name(&self) -> &'static str { "mvn" }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "mvn"
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 40 {
            return combined;
        }

        let mut out = String::new();
        let mut in_summary = false;

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("[INFO] BUILD")
                || trimmed.starts_with("[INFO] Reactor Summary")
                || trimmed.starts_with("[ERROR]")
            {
                in_summary = true;
            }
            if in_summary {
                out.push_str(trimmed);
                out.push('\n');
            }
            if trimmed.starts_with("[INFO] Total time:")
                || trimmed.starts_with("[INFO] Finished at:")
            {
                out.push_str(trimmed);
                out.push('\n');
            }
        }

        if out.trim().is_empty() {
            let status = if exit_code == 0 { "ok" } else { "FAILED" };
            out.push_str(&format!("mvn {status}\n"));
        }
        out
    }
}

pub struct GradleHandler;

impl ProxyHandler for GradleHandler {
    fn name(&self) -> &'static str { "gradle" }

    fn matches(&self, program: &str, _args: &[String]) -> bool {
        program == "gradle" || program == "gradlew" || program == "./gradlew"
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 30 {
            return combined;
        }

        let mut out = String::new();
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("BUILD ")
                || trimmed.contains("FAILED")
                || trimmed.contains("UP-TO-DATE")
                || trimmed.starts_with("> Task")
                || trimmed.starts_with("e: ")
            {
                out.push_str(trimmed);
                out.push('\n');
            }
        }

        if out.trim().is_empty() {
            let status = if exit_code == 0 { "ok" } else { "FAILED" };
            out.push_str(&format!("gradle {status}\n"));
        }
        out
    }
}

pub struct PipInstallHandler;

impl ProxyHandler for PipInstallHandler {
    fn name(&self) -> &'static str { "pip-install" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        (program == "pip" || program == "pip3" || program == "uv")
            && args.first().map(|s| s.as_str()) == Some("install")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        if lines.len() <= 20 {
            return combined;
        }

        let mut installed: Vec<&str> = Vec::new();
        let mut already: u32 = 0;

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("Successfully installed") {
                installed.push(trimmed);
            } else if trimmed.contains("already satisfied") {
                already += 1;
            }
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!("pip install {status}"));
        if already > 0 {
            out.push_str(&format!(" ({already} already satisfied)"));
        }
        out.push('\n');

        for line in &installed {
            out.push_str(&format!("  {line}\n"));
        }

        if exit_code != 0 {
            for line in &lines {
                let trimmed = line.trim();
                if trimmed.starts_with("ERROR:") || trimmed.starts_with("error:") {
                    out.push_str(&format!("  {trimmed}\n"));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_ok_short() {
        let h = MakeHandler;
        let stdout = "gcc -o main main.c\nok";
        let out = h.filter(stdout, "", 0, &[], None);
        assert!(out.contains("gcc"), "short passthrough: {out}");
    }

    #[test]
    fn make_error_long() {
        let h = MakeHandler;
        let mut stderr = String::new();
        for i in 0..40 {
            stderr.push_str(&format!("gcc -c file{i}.c\n"));
        }
        stderr.push_str("main.c:15:5: error: undeclared identifier\n");
        stderr.push_str("make: *** [Makefile:10: main] Error 1\n");
        let out = h.filter("", &stderr, 2, &[], None);
        assert!(out.contains("FAILED"), "status: {out}");
        assert!(out.contains("undeclared identifier"), "error detail: {out}");
    }

    #[test]
    fn mvn_extracts_summary() {
        let h = MvnHandler;
        let mut stdout = String::new();
        for i in 0..50 {
            stdout.push_str(&format!("[INFO] Compiling {i} source files\n"));
        }
        stdout.push_str("[INFO] BUILD SUCCESS\n");
        stdout.push_str("[INFO] Total time: 2.345 s\n");
        let out = h.filter(&stdout, "", 0, &[], None);
        assert!(out.contains("BUILD SUCCESS"), "summary: {out}");
        assert!(out.contains("Total time"), "timing: {out}");
    }

    #[test]
    fn gradle_extracts_tasks() {
        let h = GradleHandler;
        let mut stdout = String::new();
        for i in 0..40 {
            stdout.push_str(&format!("> Task :compile{i} UP-TO-DATE\n"));
        }
        stdout.push_str("BUILD SUCCESSFUL in 5s\n");
        let out = h.filter(&stdout, "", 0, &[], None);
        assert!(out.contains("BUILD SUCCESSFUL"), "summary: {out}");
    }

    #[test]
    fn pip_install_summary() {
        let h = PipInstallHandler;
        let mut stdout = String::new();
        for i in 0..30 {
            stdout.push_str(&format!("Requirement already satisfied: pkg{i}\n"));
        }
        stdout.push_str("Successfully installed newpkg-1.0\n");
        let out = h.filter(&stdout, "", 0, &[], None);
        assert!(out.contains("pip install ok"), "status: {out}");
        assert!(out.contains("30 already satisfied"), "already count: {out}");
        assert!(out.contains("Successfully installed"), "installed: {out}");
    }

    #[test]
    fn matches_build_tools() {
        assert!(MakeHandler.matches("make", &[]));
        assert!(MakeHandler.matches("gmake", &[]));
        assert!(MvnHandler.matches("mvn", &[]));
        assert!(GradleHandler.matches("gradle", &[]));
        assert!(GradleHandler.matches("gradlew", &[]));
        assert!(PipInstallHandler.matches("pip", &[String::from("install")]));
        assert!(PipInstallHandler.matches("pip3", &[String::from("install")]));
        assert!(PipInstallHandler.matches("uv", &[String::from("install")]));
        assert!(!PipInstallHandler.matches("pip", &[String::from("freeze")]));
    }
}
