//! Command handlers: structural parsing + compression for high-value commands.
//!
//! Each handler implements `ProxyHandler` and is registered in `HANDLERS`.
//! The router tries handlers in order; first match wins. Unmatched commands
//! fall through to the TOML declarative filter engine.

pub mod cargo;
pub mod git;
pub mod go;
pub mod js;
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
