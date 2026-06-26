//! Bash / Zsh function-call extractor.
//!
//! tree-sitter-bash represents a function invocation as a `command` node
//! whose `name` field holds the invoked identifier. Definitions land as
//! `function_definition` with a `name` field — the chunker already picks
//! those up as function symbols, so we only need to emit Call refs on
//! the invocation side here.
//!
//! Builtin filter: shell scripts are dense with `echo`/`cd`/`grep`/etc.
//! Letting those flow into the call graph drowns the user's real
//! function-to-function edges in noise. The list below covers common
//! POSIX/coreutils/sh-internal commands plus a handful of standard tools
//! that show up in nearly every CI script.
//!
//! Out of scope (intentional, kept honest in docs):
//!   * Pipes, subshells, here-docs — invocation site is still a `command`,
//!     so we'll pick it up. The pipe operator itself is not a call.
//!   * `source ./lib.sh` / `. ./lib.sh` — emitted as Call against `source`
//!     / `.` and then filtered by the builtin list. Cross-file binding
//!     between caller.sh and lib.sh stays at the L2 / L4 tiers.

use crate::graph::extractor::{
    LanguageRefExtractor, RawReference, RefKind, build_scope_map, default_scope_name,
};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

pub struct BashExtractor;

impl LanguageRefExtractor for BashExtractor {
    fn extract(&self, tree: &Tree, source: &str) -> Vec<RawReference> {
        let mut refs = Vec::new();
        let root = tree.root_node();
        let scope_map =
            build_scope_map(&root, source, &["function_definition"], default_scope_name);
        collect_refs(&root, source, &scope_map, &mut refs);
        refs
    }

    fn is_test_file(&self, path: &str) -> bool {
        // Common patterns: `tests/foo.sh`, `test/foo.sh`, `foo_test.sh`,
        // bats `*.bats`. Keep the matcher lenient — false positives here
        // just mean a real call site gets filtered out of agent-facing
        // listings, not silent data loss.
        let name = path.rsplit('/').next().unwrap_or(path);
        path.contains("/tests/")
            || path.contains("/test/")
            || name.ends_with("_test.sh")
            || name.ends_with("_test.bash")
            || name.ends_with(".bats")
    }

    fn resolve_import(&self, _import_path: &str, _workspace: &Path) -> Option<PathBuf> {
        None
    }

    fn language_name(&self) -> &'static str {
        "bash"
    }
}

fn collect_refs(
    node: &Node,
    source: &str,
    scope_map: &[Option<String>],
    refs: &mut Vec<RawReference>,
) {
    if node.kind() == "command"
        && let Some(name_node) = node.child_by_field_name("name")
    {
        let name = text(&name_node, source);
        // The grammar exposes the invoked command's "name" field as the
        // expanded word — variable expansions, command substitutions,
        // and `$(...)` interpolations all show up as non-identifier
        // word nodes. We only emit when the head is a plain identifier
        // so the call graph stays honest. Same convention as the other
        // extractors' identifier-only Call branch.
        if is_plain_ident(&name) && !is_bash_builtin(&name) {
            let line = name_node.start_position().row as u32;
            let enclosing = scope_map.get(line as usize).and_then(|s| s.clone());
            refs.push(RawReference {
                line,
                source_symbol: enclosing,
                target_name: name,
                target_qualifier: None,
                receiver_type: None,
                kind: RefKind::Call,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(&child, source, scope_map, refs);
    }
}

fn text(node: &Node, source: &str) -> String {
    source[node.start_byte()..node.end_byte()].to_string()
}

/// Bare identifier in the Bash sense: starts with letter or `_`, then
/// letters / digits / `_`. Rejects `$var`, `$(cmd)`, glob expansions —
/// anything that isn't a literal function name.
fn is_plain_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Block-list of common builtins / coreutils / sh-shell tooling so the
/// call graph doesn't drown in `echo` / `cd` / `git` edges. The patch
/// brief enumerates these — keep this list and that brief in sync if
/// either expands.
fn is_bash_builtin(name: &str) -> bool {
    matches!(
        name,
        "echo"
            | "cd"
            | "ls"
            | "mkdir"
            | "rm"
            | "ln"
            | "mv"
            | "cp"
            | "find"
            | "grep"
            | "sed"
            | "awk"
            | "cat"
            | "head"
            | "tail"
            | "sort"
            | "uniq"
            | "wc"
            | "set"
            | "export"
            | "source"
            | "exit"
            | "return"
            | "true"
            | "false"
            | "test"
            | "["
            | "[["
            | "read"
            | "printf"
            | "eval"
            | "exec"
            | "shift"
            | "trap"
            | "kill"
            | "jobs"
            | "fg"
            | "bg"
            | "wait"
            | "sleep"
            | "date"
            | "dirname"
            | "basename"
            | "realpath"
            | "mktemp"
            | "tar"
            | "curl"
            | "wget"
            | "git"
            | "cargo"
            | "npm"
            | "node"
            | "python"
            | "bash"
            | "sh"
            | "zsh"
    )
}
