//! Kubernetes handlers: kubectl get, kubectl describe, kubectl logs.

use super::{ProxyContext, ProxyHandler};

pub struct KubectlGetHandler;

impl ProxyHandler for KubectlGetHandler {
    fn name(&self) -> &'static str { "kubectl-get" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "kubectl" && args.first().map(|s| s.as_str()) == Some("get")
    }

    fn filter(&self, stdout: &str, _stderr: &str, _exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 30 {
            return stdout.to_string();
        }

        let mut out = String::new();
        // Keep header
        if let Some(header) = lines.first() {
            out.push_str(header);
            out.push('\n');
        }

        // Show first 25 rows + count
        let data_lines = &lines[1..];
        for line in data_lines.iter().take(25) {
            // Truncate wide lines
            if line.len() > 150 {
                out.push_str(&line[..150]);
                out.push_str("...\n");
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        if data_lines.len() > 25 {
            out.push_str(&format!("... {} more rows\n", data_lines.len() - 25));
        }
        out
    }
}

pub struct KubectlLogsHandler;

impl ProxyHandler for KubectlLogsHandler {
    fn name(&self) -> &'static str { "kubectl-logs" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "kubectl" && args.first().map(|s| s.as_str()) == Some("logs")
    }

    fn filter(&self, stdout: &str, _stderr: &str, _exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 50 {
            return stdout.to_string();
        }

        // Deduplicate repeated log lines
        let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut unique_lines: Vec<(String, u32)> = Vec::new();

        for line in &lines {
            // Normalize: strip timestamp prefix for dedup
            let key = strip_log_timestamp(line);
            *seen.entry(key.clone()).or_default() += 1;
            if seen[&key] == 1 {
                unique_lines.push((line.to_string(), 0));
            }
        }

        // Update counts
        for (line, count) in &mut unique_lines {
            let key = strip_log_timestamp(line);
            *count = seen[&key];
        }

        let mut out = String::new();
        out.push_str(&format!("{} log lines ({} unique)\n\n", lines.len(), unique_lines.len()));

        let show = unique_lines.len().min(40);
        for (line, count) in unique_lines.iter().take(show) {
            if *count > 1 {
                out.push_str(&format!("{line}  (×{count})\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        if unique_lines.len() > 40 {
            out.push_str(&format!("... {} more unique lines\n", unique_lines.len() - 40));
        }
        out
    }
}

fn strip_log_timestamp(line: &str) -> String {
    let trimmed = line.trim();
    // ISO timestamps: 2024-01-15T10:30:45.123Z
    if trimmed.len() > 24 && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(10) == Some(&b'T')
    {
        return trimmed[24..].trim().to_string();
    }
    // Syslog: "Jan 15 10:30:45" (15 chars)
    if trimmed.len() > 16 && trimmed.as_bytes().get(3) == Some(&b' ')
        && trimmed.as_bytes().get(6) == Some(&b' ')
        && let Some(rest) = trimmed.get(16..)
    {
        return rest.trim().to_string();
    }
    trimmed.to_string()
}
