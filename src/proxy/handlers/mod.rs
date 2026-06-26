//! Command handlers: structural parsing + compression for high-value commands.
//!
//! Each handler implements `ProxyHandler` and is registered in `HANDLERS`.
//! The router tries handlers in order; first match wins. Unmatched commands
//! fall through to the TOML declarative filter engine.

pub mod build;
pub mod cargo;
pub mod docker;
pub mod git;
pub mod go;
pub mod infra;
pub mod js;
pub mod kubectl;
pub mod lint;
pub mod python;
pub mod system;

use super::ProxyContext;

pub trait ProxyHandler: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, program: &str, args: &[String]) -> bool;
    fn filter(&self, stdout: &str, stderr: &str, exit_code: i32, args: &[String], ctx: Option<&ProxyContext>) -> String;
}

/// Registry of all built-in handlers, checked in order.
pub fn all_handlers() -> Vec<Box<dyn ProxyHandler>> {
    vec![
        Box::new(git::GitStatusHandler),
        Box::new(git::GitDiffHandler),
        Box::new(git::GitLogHandler),
        Box::new(cargo::CargoBuildHandler),
        Box::new(cargo::CargoTestHandler),
        Box::new(cargo::CargoClippyHandler),
        Box::new(go::GoTestHandler),
        Box::new(go::GoBuildHandler),
        Box::new(js::NpmTestHandler),
        Box::new(python::PytestHandler),
        Box::new(docker::DockerComposeHandler),
        Box::new(docker::DockerPsHandler),
        Box::new(kubectl::KubectlGetHandler),
        Box::new(kubectl::KubectlLogsHandler),
        Box::new(lint::EslintHandler),
        Box::new(lint::RuffHandler),
        Box::new(lint::TscHandler),
        Box::new(infra::TerraformPlanHandler),
        Box::new(infra::TerraformApplyHandler),
        Box::new(infra::HelmHandler),
        Box::new(build::MakeHandler),
        Box::new(build::MvnHandler),
        Box::new(build::GradleHandler),
        Box::new(build::PipInstallHandler),
        Box::new(system::LsHandler),
        Box::new(system::FindHandler),
        Box::new(system::GrepHandler),
        Box::new(system::CatHandler),
    ]
}

/// Find the first handler that matches the command.
pub fn find_handler<'a>(
    handlers: &'a [Box<dyn ProxyHandler>],
    program: &str,
    args: &[String],
) -> Option<&'a dyn ProxyHandler> {
    handlers
        .iter()
        .find(|h| h.matches(program, args))
        .map(|h| h.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_handlers_returns_nonempty() {
        let handlers = all_handlers();
        assert!(handlers.len() >= 28, "expected ≥28 handlers, got {}", handlers.len());
    }

    #[test]
    fn find_handler_routes_correctly() {
        let handlers = all_handlers();
        let cases: &[(&str, &[&str], &str)] = &[
            ("git", &["status"], "git-status"),
            ("git", &["diff"], "git-diff"),
            ("git", &["log"], "git-log"),
            ("cargo", &["build"], "cargo-build"),
            ("cargo", &["test"], "cargo-test"),
            ("cargo", &["clippy"], "cargo-clippy"),
            ("go", &["test", "./..."], "go-test"),
            ("go", &["build"], "go-build"),
            ("npm", &["test"], "npm-test"),
            ("pytest", &[], "pytest"),
            ("docker", &["compose", "up"], "docker-compose"),
            ("docker", &["ps"], "docker-ps"),
            ("kubectl", &["get", "pods"], "kubectl-get"),
            ("kubectl", &["logs", "pod-1"], "kubectl-logs"),
            ("eslint", &["."], "eslint"),
            ("ruff", &["check", "."], "ruff"),
            ("tsc", &[], "tsc"),
            ("ls", &["-la"], "ls"),
            ("find", &[".", "-name", "*.rs"], "find"),
            ("grep", &["-rn", "foo"], "grep"),
            ("cat", &["file.rs"], "cat"),
            ("terraform", &["plan"], "terraform-plan"),
            ("terraform", &["apply"], "terraform-apply"),
            ("helm", &["install", "myapp"], "helm"),
            ("make", &[], "make"),
            ("mvn", &["package"], "mvn"),
            ("gradle", &["build"], "gradle"),
            ("pip", &["install", "requests"], "pip-install"),
        ];
        for (program, args, expected_name) in cases {
            let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let handler = find_handler(&handlers, program, &args_owned);
            assert!(
                handler.is_some(),
                "no handler matched for `{program} {}`", args.join(" ")
            );
            assert_eq!(
                handler.unwrap().name(), *expected_name,
                "wrong handler for `{program} {}`", args.join(" ")
            );
        }
    }

    #[test]
    fn find_handler_returns_none_for_unknown() {
        let handlers = all_handlers();
        let args: Vec<String> = vec![];
        assert!(find_handler(&handlers, "some-unknown-cmd", &args).is_none());
    }
}
