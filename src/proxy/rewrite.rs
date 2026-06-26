//! Command rewrite: translate raw shell commands into `abyss proxy` form.
//!
//! Used by the hook scripts to intercept agent commands.
//! `abyss rewrite <command>` outputs the rewritten command on stdout.

/// Known commands that have proxy handlers or TOML filters.
const REWRITE_PREFIXES: &[&str] = &[
    "git", "cargo", "npm", "pnpm", "yarn", "npx", "pytest", "python", "python3",
    "go", "make", "docker", "kubectl", "pip", "pip3",
    "eslint", "ruff", "tsc", "mypy", "flake8",
    "ls", "find", "grep", "rg", "ag", "cat", "head", "tail", "wc",
];

/// Commands that should never be rewritten (interactive, destructive, etc.).
const IGNORED_EXACT: &[&str] = &[
    "cd", "exit", "vim", "vi", "nano", "emacs", "htop", "top", "less",
    "more", "man", "ssh", "scp", "rsync", "abyss",
];

/// Rewrite a shell command string for proxy interception.
///
/// Returns `Some(rewritten)` if the command should be proxied,
/// `None` if it should pass through unmodified.
pub fn rewrite_command(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Already proxied
    if trimmed.starts_with("abyss proxy ") || trimmed.starts_with("abyss ") {
        return None;
    }

    // Handle compound commands: split on && || ;
    if trimmed.contains("&&") || trimmed.contains("||") || trimmed.contains(';') {
        return rewrite_compound(trimmed);
    }

    // Unattestable constructs — pass through (anti-bypass, same as RTK)
    if trimmed.contains("$(") || trimmed.contains('`') || trimmed.contains("<<") {
        return None;
    }

    // Extract program name
    let program = trimmed.split_whitespace().next()?;

    // Check ignored
    if IGNORED_EXACT.contains(&program) {
        return None;
    }

    // Check if rewritable
    if REWRITE_PREFIXES.contains(&program) {
        // Special case: cat → abyss proxy cat (smart read potential)
        // Special case: head -N file → abyss proxy head (preserved as-is)
        return Some(format!("abyss proxy {trimmed}"));
    }

    None
}

fn rewrite_compound(cmd: &str) -> Option<String> {
    // Split on && while preserving the operator
    let mut parts = Vec::new();
    let mut any_rewritten = false;

    // Simple split — handles && and ; but not nested parens
    for segment in cmd.split("&&") {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }

        // Further split on ;
        let sub_parts: Vec<&str> = seg.split(';').collect();
        let mut rewritten_subs = Vec::new();

        for sub in sub_parts {
            let sub = sub.trim();
            if sub.is_empty() {
                continue;
            }
            if let Some(r) = rewrite_command(sub) {
                rewritten_subs.push(r);
                any_rewritten = true;
            } else {
                rewritten_subs.push(sub.to_string());
            }
        }
        parts.push(rewritten_subs.join("; "));
    }

    if any_rewritten {
        Some(parts.join(" && "))
    } else {
        None
    }
}

/// Generate the hook-response JSON for Claude Code's PreToolUse hook.
///
/// Exit-code protocol (compatible with RTK):
/// - 0: rewrite, auto-allow
/// - 1: no equivalent, passthrough
pub fn hook_response_claude(original_input: &serde_json::Value, rewritten_cmd: &str) -> String {
    let mut updated_input = original_input.clone();
    if let Some(obj) = updated_input.as_object_mut() {
        obj.insert("command".into(), serde_json::Value::String(rewritten_cmd.into()));
    }

    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "abyss proxy rewrite",
            "updatedInput": updated_input
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_basic() {
        assert_eq!(
            rewrite_command("git status"),
            Some("abyss proxy git status".into())
        );
        assert_eq!(
            rewrite_command("cargo test"),
            Some("abyss proxy cargo test".into())
        );
        assert_eq!(
            rewrite_command("ls -la src/"),
            Some("abyss proxy ls -la src/".into())
        );
    }

    #[test]
    fn rewrite_already_proxied() {
        assert_eq!(rewrite_command("abyss proxy git status"), None);
        assert_eq!(rewrite_command("abyss hook pre-edit"), None);
    }

    #[test]
    fn rewrite_ignored() {
        assert_eq!(rewrite_command("vim file.rs"), None);
        assert_eq!(rewrite_command("ssh server"), None);
    }

    #[test]
    fn rewrite_unknown() {
        assert_eq!(rewrite_command("custom-tool --flag"), None);
    }

    #[test]
    fn rewrite_compound() {
        let result = rewrite_command("git status && cargo test");
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.contains("abyss proxy git status"));
        assert!(r.contains("abyss proxy cargo test"));
    }

    #[test]
    fn rewrite_unattestable() {
        assert_eq!(rewrite_command("echo $(git status)"), None);
        assert_eq!(rewrite_command("cat <<EOF"), None);
    }
}
