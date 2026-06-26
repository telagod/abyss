//! Infrastructure tool handlers: terraform, ansible, helm.

use super::{ProxyContext, ProxyHandler};

pub struct TerraformPlanHandler;

impl ProxyHandler for TerraformPlanHandler {
    fn name(&self) -> &'static str {
        "terraform-plan"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "terraform" && args.first().map(|s| s.as_str()) == Some("plan")
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut adds = 0u32;
        let mut changes = 0u32;
        let mut destroys = 0u32;
        let mut resource_actions: Vec<String> = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("# ")
                && (trimmed.contains("will be") || trimmed.contains("must be"))
            {
                resource_actions.push(trimmed.to_string());
            }
            if trimmed.contains("Plan:") {
                for word in trimmed.split_whitespace() {
                    if let Ok(n) = word.parse::<u32>() {
                        let rest = &trimmed[trimmed.find(word).unwrap_or(0)..];
                        if rest.contains("to add") {
                            adds = n;
                        }
                        if rest.contains("to change") {
                            changes = n;
                        }
                        if rest.contains("to destroy") {
                            destroys = n;
                        }
                    }
                }
            }
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!(
            "terraform plan {status}: +{adds} ~{changes} -{destroys}\n"
        ));

        for action in resource_actions.iter().take(20) {
            out.push_str(&format!("  {action}\n"));
        }
        if resource_actions.len() > 20 {
            out.push_str(&format!(
                "  ... {} more resources\n",
                resource_actions.len() - 20
            ));
        }
        out
    }
}

pub struct TerraformApplyHandler;

impl ProxyHandler for TerraformApplyHandler {
    fn name(&self) -> &'static str {
        "terraform-apply"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "terraform" && args.first().map(|s| s.as_str()) == Some("apply")
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        _args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();

        let mut out = String::new();
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("Apply complete!")
                || trimmed.starts_with("Error:")
                || (trimmed.contains("created")
                    || trimmed.contains("destroyed")
                    || trimmed.contains("modified"))
                    && !trimmed.is_empty()
            {
                out.push_str(trimmed);
                out.push('\n');
            }
        }

        if out.trim().is_empty() {
            let status = if exit_code == 0 { "ok" } else { "FAILED" };
            out.push_str(&format!("terraform apply {status}\n"));
        }
        out
    }
}

pub struct HelmHandler;

impl ProxyHandler for HelmHandler {
    fn name(&self) -> &'static str {
        "helm"
    }

    fn matches(&self, program: &str, args: &[String]) -> bool {
        program == "helm"
            && args
                .first()
                .map(|s| s.as_str())
                .is_some_and(|a| matches!(a, "install" | "upgrade" | "list" | "status"))
    }

    fn filter(
        &self,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        args: &[String],
        _ctx: Option<&ProxyContext>,
    ) -> String {
        let combined = format!("{stdout}\n{stderr}");
        let lines: Vec<&str> = combined.lines().collect();
        let subcommand = args.first().map(|s| s.as_str()).unwrap_or("?");

        if lines.len() <= 30 {
            return combined;
        }

        let mut out = String::new();
        let status = if exit_code == 0 { "ok" } else { "FAILED" };
        out.push_str(&format!("helm {subcommand} {status}\n"));

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("NAME:")
                || trimmed.starts_with("STATUS:")
                || trimmed.starts_with("REVISION:")
                || trimmed.starts_with("NAMESPACE:")
                || trimmed.starts_with("LAST DEPLOYED:")
                || trimmed.starts_with("NOTES:")
                || trimmed.contains("has been")
            {
                out.push_str(&format!("  {trimmed}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terraform_plan_summary() {
        let h = TerraformPlanHandler;
        let stdout = "\
Terraform will perform the following actions:

  # aws_instance.web will be created
  # aws_s3_bucket.data will be created
  # aws_security_group.default will be updated in-place

Plan: 2 to add, 1 to change, 0 to destroy.";
        let out = h.filter(stdout, "", 0, &[], None);
        assert!(out.contains("+2"), "adds: {out}");
        assert!(out.contains("~1"), "changes: {out}");
        assert!(out.contains("-0"), "destroys: {out}");
        assert!(out.contains("will be created"), "resource: {out}");
    }

    #[test]
    fn terraform_apply_ok() {
        let h = TerraformApplyHandler;
        let stdout = "\
aws_instance.web: Creating...
aws_instance.web: Creation complete after 30s [id=i-abc123]

Apply complete! Resources: 1 added, 0 changed, 0 destroyed.";
        let out = h.filter(stdout, "", 0, &[], None);
        assert!(out.contains("Apply complete!"), "summary: {out}");
    }

    #[test]
    fn helm_short_passthrough() {
        let h = HelmHandler;
        let stdout = "NAME: myapp\nSTATUS: deployed\n";
        let out = h.filter(stdout, "", 0, &[String::from("status")], None);
        assert!(out.contains("NAME: myapp"), "short passthrough: {out}");
    }

    #[test]
    fn matches_terraform_helm() {
        let tp = TerraformPlanHandler;
        assert!(tp.matches("terraform", &[String::from("plan")]));
        assert!(!tp.matches("terraform", &[String::from("init")]));
        let ta = TerraformApplyHandler;
        assert!(ta.matches("terraform", &[String::from("apply")]));
        let hh = HelmHandler;
        assert!(hh.matches("helm", &[String::from("install")]));
        assert!(hh.matches("helm", &[String::from("upgrade")]));
        assert!(!hh.matches("helm", &[String::from("repo")]));
    }
}
