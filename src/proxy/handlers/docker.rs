//! Docker handlers: docker compose, docker build, docker ps.

use super::{ProxyContext, ProxyHandler};

pub struct DockerComposeHandler;

impl ProxyHandler for DockerComposeHandler {
    fn name(&self) -> &'static str { "docker-compose" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "docker" && args.first().map(|s| s.as_str()) == Some("compose")
    }

    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        // docker compose up/down/ps — strip progress bars and pull output
        let mut out = String::new();
        let mut containers: Vec<&str> = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Skip pull progress, layer downloads, digest lines
            if trimmed.starts_with("Pulling ")
                || trimmed.starts_with("Downloading")
                || trimmed.starts_with("Extracting")
                || trimmed.starts_with("Waiting")
                || trimmed.starts_with("Digest:")
                || trimmed.starts_with("Status:")
                || trimmed.contains("Pull complete")
                || trimmed.contains("Already exists")
            {
                continue;
            }
            // Capture container status lines
            if trimmed.contains("Started") || trimmed.contains("Running")
                || trimmed.contains("Stopped") || trimmed.contains("Created")
                || trimmed.contains("Healthy") || trimmed.contains("Exited")
            {
                containers.push(trimmed);
                continue;
            }
            out.push_str(trimmed);
            out.push('\n');
        }

        if !containers.is_empty() {
            out.push_str(&format!("\ncontainers ({}):\n", containers.len()));
            for c in &containers {
                out.push_str(&format!("  {c}\n"));
            }
        }

        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        if out.trim().is_empty() {
            return format!("docker compose {status}\n");
        }
        out
    }
}

pub struct DockerPsHandler;

impl ProxyHandler for DockerPsHandler {
    fn name(&self) -> &'static str { "docker-ps" }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "docker" && args.first().map(|s| s.as_str()) == Some("ps")
    }

    fn filter(&self, stdout: &str, _stderr: &str, _exit_code: i32, _args: &[String], _ctx: Option<&ProxyContext>) -> String {
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() <= 20 {
            return stdout.to_string();
        }

        // Keep header + truncate columns
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                out.push_str(line);
                out.push('\n');
                continue;
            }
            // Truncate wide lines (PORTS column is usually huge noise)
            if line.len() > 120 {
                out.push_str(&line[..120]);
                out.push_str("...\n");
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}
