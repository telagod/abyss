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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_strips_pull_progress() {
        let h = DockerComposeHandler;
        let stderr = "\
Pulling web (node:18)...
Downloading [==>        ] 12.3MB/45.6MB
Extracting  [====>      ]
Digest: sha256:abc123
Status: Downloaded newer image
Pull complete
web-1  | Started
db-1   | Running";
        let out = h.filter("", stderr, 0, &[], None);
        assert!(!out.contains("Pulling"), "should strip pull: {out}");
        assert!(!out.contains("Downloading"), "should strip download: {out}");
        assert!(!out.contains("Digest:"), "should strip digest: {out}");
        assert!(out.contains("containers (2):"), "container summary: {out}");
        assert!(out.contains("Started"), "should keep status: {out}");
    }

    #[test]
    fn compose_empty_output() {
        let h = DockerComposeHandler;
        let out = h.filter("", "", 0, &[], None);
        assert!(out.contains("docker compose ok"), "empty = ok: {out}");
    }

    #[test]
    fn docker_ps_small_passthrough() {
        let h = DockerPsHandler;
        let stdout = "CONTAINER ID   IMAGE   STATUS\nabc123   nginx   Up 5m\n";
        let out = h.filter(stdout, "", 0, &[], None);
        assert_eq!(out, stdout);
    }

    #[test]
    fn docker_ps_truncates_wide_lines() {
        let h = DockerPsHandler;
        let header = "CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES\n";
        let mut stdout = header.to_string();
        for i in 0..25 {
            let wide = format!("abc{i:03}   nginx   \"nginx -g 'daemon off;'\"   2 hours ago   Up 2 hours   0.0.0.0:{}->{}/tcp, :::{}->{}   web-server-{i}\n",
                8080 + i, 80 + i, 8080 + i, 80 + i);
            stdout.push_str(&wide);
        }
        let out = h.filter(&stdout, "", 0, &[], None);
        assert!(out.contains("..."), "should truncate wide: {out}");
    }

    #[test]
    fn compose_matches() {
        let h = DockerComposeHandler;
        assert!(h.matches("docker", &[String::from("compose")]));
        assert!(!h.matches("docker", &[String::from("run")]));
    }
}
