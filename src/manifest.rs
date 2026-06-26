//! Machine-readable skill manifest emitted by `abyss skill-manifest`.
//!
//! The companion `code-abyss` package (and any other skill-discovery
//! consumer) reads this to know what abyss exposes — CLI commands, MCP
//! tools, hook entry points, daemon socket verbs — without hand-coding
//! the integration.
//!
//! `schema_version` is a single integer so consumers can detect format
//! changes without a semver dance on the abyss release. Bump it any
//! time a field is renamed, removed, or its semantics change.

use serde_json::{Value, json};

/// Current manifest schema version. Bumped on breaking shape changes only.
pub const SCHEMA_VERSION: u32 = 1;

/// Build the manifest as a `serde_json::Value`. Kept as a pure builder
/// so unit tests can assert structural invariants without spawning the
/// binary or touching stdout.
pub fn build_manifest() -> Value {
    json!({
        "name": "abyss",
        "version": env!("CARGO_PKG_VERSION"),
        "kind": "code-graph",
        "description": "The code graph your agent checks before it edits.",
        "homepage": "https://telagod.github.io/abyss/",
        "repo": "https://github.com/telagod/abyss",
        "providers": {
            "cli": {
                "binary": "abyss",
                "commands": cli_commands(),
            },
            "mcp": {
                "command": "abyss mcp",
                "via_daemon": "abyss mcp --via-daemon",
                "tools": mcp_tools(),
            },
            "hooks": {
                "pre_edit": "abyss hook pre-edit",
                "post_edit": "abyss hook post-edit",
                "attach": {
                    "claude":   "abyss attach claude",
                    "codex":    "abyss attach codex",
                    "gemini":   "abyss attach gemini",
                    // v0.5.23: OpenClaw downgraded to a no-op with a clear
                    // migration message. Sister code-abyss adapter uses a
                    // per-pack install layout (packs/abyss/openclaw/),
                    // which abyss attach cannot replicate from a single
                    // binary. Keep the key in the manifest so consumers
                    // know the command exists, but advertise the real
                    // alternative via `note`.
                    "openclaw": "abyss attach openclaw",
                    "all":      "abyss attach all",
                },
                "attach_notes": {
                    "openclaw": "no-op in v0.5.23+: OpenClaw uses a per-pack install layout. Use `npx code-abyss -t openclaw --with-abyss` instead.",
                },
            },
            "daemon": {
                "socket": ".code-abyss/daemon.sock",
                "verbs":  ["ping", "stats", "reindex", "logs", "mcp", "subscribe"],
            },
        },
        "schema_version": SCHEMA_VERSION,
    })
}

fn cli_commands() -> Value {
    json!([
        { "name": "index",          "summary": "Build call graph + temporal index" },
        { "name": "context",        "summary": "Pre-edit context for a file" },
        { "name": "callers",        "summary": "Who calls/uses a symbol" },
        { "name": "impact",         "summary": "Blast radius analysis" },
        { "name": "where",          "summary": "File's coordinates in the codebase" },
        { "name": "history",        "summary": "Per-file (or per-symbol) git history with churn + coupling" },
        { "name": "search",         "summary": "Symbol + fulltext fusion search" },
        { "name": "map",            "summary": "Codebase map: hotspots, coupling, risk areas" },
        { "name": "stats",          "summary": "Index statistics" },
        { "name": "mcp",            "summary": "Run as MCP server (stdio transport)" },
        { "name": "watch",          "summary": "Foreground daemon-lite: reindex on save" },
        { "name": "daemon",         "summary": "Background daemon (Unix): start/stop/status/logs" },
        { "name": "hook",           "summary": "Agent hook entry points (pre-edit / post-edit)" },
        { "name": "attach",         "summary": "Install hooks into a host's settings (idempotent)" },
        { "name": "completion",     "summary": "Print a shell-completion script" },
        { "name": "config",         "summary": "Inspect the effective abyss config" },
        { "name": "reset",          "summary": "Clean .code-abyss/ surfaces" },
        { "name": "ingest",         "summary": "Ingest SCIP ground-truth (prototype)" },
        { "name": "skill-manifest", "summary": "Emit this manifest (machine-readable JSON)" },
    ])
}

fn mcp_tools() -> Value {
    json!([
        "search_context",
        "get_symbols",
        "find_callers",
        "impact_analysis",
        "code_map",
        "evolution",
        "index_project",
        "arch_map",
        "proxy_gain",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_required_top_level_keys() {
        let v = build_manifest();
        for key in [
            "name",
            "version",
            "kind",
            "description",
            "homepage",
            "repo",
            "providers",
            "schema_version",
        ] {
            assert!(v.get(key).is_some(), "missing key: {key}");
        }
    }

    #[test]
    fn schema_version_is_v1() {
        let v = build_manifest();
        assert_eq!(v["schema_version"].as_u64(), Some(SCHEMA_VERSION as u64));
    }

    #[test]
    fn cli_commands_non_empty_and_well_formed() {
        let v = build_manifest();
        let cmds = v["providers"]["cli"]["commands"].as_array().unwrap();
        assert!(!cmds.is_empty());
        for c in cmds {
            assert!(c["name"].as_str().is_some());
            assert!(c["summary"].as_str().is_some());
        }
    }

    #[test]
    fn mcp_tools_cover_full_surface() {
        let v = build_manifest();
        let tools = v["providers"]["mcp"]["tools"].as_array().unwrap();
        assert!(
            tools.len() >= 7,
            "expected >=7 MCP tools, got {}",
            tools.len()
        );
        // The 7 historical tools must still be advertised; arch_map is the
        // newer addition and may or may not be there depending on build.
        for expected in [
            "search_context",
            "get_symbols",
            "find_callers",
            "impact_analysis",
            "code_map",
            "evolution",
            "index_project",
        ] {
            assert!(
                tools.iter().any(|t| t.as_str() == Some(expected)),
                "missing MCP tool: {expected}"
            );
        }
    }

    #[test]
    fn version_matches_cargo() {
        let v = build_manifest();
        assert_eq!(v["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn attach_lists_all_four_hosts() {
        let v = build_manifest();
        let attach = &v["providers"]["hooks"]["attach"];
        for host in ["claude", "codex", "gemini", "openclaw", "all"] {
            assert!(
                attach.get(host).and_then(Value::as_str).is_some(),
                "missing attach host: {host}"
            );
        }
    }
}
