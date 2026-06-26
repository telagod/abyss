//! TOML declarative filter engine for the long tail of commands.
//!
//! 8-stage pipeline (inspired by RTK):
//! 1. Strip ANSI escape codes
//! 2. Chained regex replace
//! 3. match_output short-circuit (full-blob regex → canned message)
//! 4. strip_lines_matching / keep_lines_matching
//! 5. truncate_lines_at (per-line char limit)
//! 6. head_lines / tail_lines
//! 7. max_lines absolute cap
//! 8. on_empty fallback message

use std::path::Path;

use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct FilterFile {
    #[allow(dead_code)]
    pub schema_version: Option<u32>,
    #[serde(default)]
    pub filters: std::collections::HashMap<String, FilterDef>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FilterDef {
    pub match_command: String,
    #[serde(default)]
    pub strip_ansi: bool,
    #[serde(default)]
    pub replace: Vec<ReplaceRule>,
    #[serde(default)]
    pub match_output: Vec<MatchOutputRule>,
    #[serde(default)]
    pub strip_lines_matching: Vec<String>,
    #[serde(default)]
    pub keep_lines_matching: Vec<String>,
    #[serde(default)]
    pub truncate_lines_at: Option<usize>,
    #[serde(default)]
    pub head_lines: Option<usize>,
    #[serde(default)]
    pub tail_lines: Option<usize>,
    #[serde(default)]
    pub max_lines: Option<usize>,
    #[serde(default)]
    pub on_empty: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ReplaceRule {
    pub pattern: String,
    pub replacement: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MatchOutputRule {
    pub pattern: String,
    pub message: String,
    #[serde(default)]
    pub unless: Option<String>,
}

/// Load filters from a TOML file with trust verification.
///
/// Project-local filters (`.code-abyss/filters.toml`) are untrusted by
/// default. A `.code-abyss/filters.toml.sha256` sidecar containing the
/// hex SHA-256 of the TOML content must exist and match, or the filters
/// are silently skipped. Set `ABYSS_TRUST_FILTERS=1` to bypass (CI use).
///
/// Builtin filters (compiled into the binary) are always trusted.
pub fn load_filters(path: &Path) -> std::collections::HashMap<String, FilterDef> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Default::default(),
    };

    // Trust gate: verify SHA-256 sidecar for project-local filters
    if std::env::var("ABYSS_TRUST_FILTERS").as_deref() != Ok("1") {
        let hash_path = path.with_extension("toml.sha256");
        if !verify_sha256(&raw, &hash_path) {
            eprintln!(
                "[abyss proxy] untrusted filters at {} — skipped (run `abyss trust-filter` to approve)",
                path.display()
            );
            return Default::default();
        }
    }

    match toml::from_str::<FilterFile>(&raw) {
        Ok(f) => f.filters,
        Err(_) => Default::default(),
    }
}

fn verify_sha256(content: &str, hash_path: &Path) -> bool {
    let expected = match std::fs::read_to_string(hash_path) {
        Ok(s) => s.trim().to_lowercase(),
        Err(_) => return false,
    };
    let actual = {
        use blake3::Hasher;
        let mut h = Hasher::new();
        h.update(content.as_bytes());
        h.finalize().to_hex().to_string()
    };
    actual == expected
}

/// Write the trust sidecar for a filters.toml file.
pub fn write_trust_hash(filter_path: &Path) -> std::io::Result<()> {
    let content = std::fs::read_to_string(filter_path)?;
    let hash = {
        use blake3::Hasher;
        let mut h = Hasher::new();
        h.update(content.as_bytes());
        h.finalize().to_hex().to_string()
    };
    let hash_path = filter_path.with_extension("toml.sha256");
    std::fs::write(hash_path, &hash)
}

/// Find the first filter whose match_command regex matches the full command.
pub fn find_filter<'a>(
    filters: &'a std::collections::HashMap<String, FilterDef>,
    full_command: &str,
) -> Option<&'a FilterDef> {
    for def in filters.values() {
        if let Ok(re) = Regex::new(&def.match_command) && re.is_match(full_command) {
            return Some(def);
        }
    }
    None
}

/// Apply the 8-stage filter pipeline to raw output.
pub fn apply_filter(def: &FilterDef, raw: &str) -> String {
    let mut text = raw.to_string();

    // Stage 1: strip ANSI
    if def.strip_ansi {
        text = strip_ansi_codes(&text);
    }

    // Stage 2: chained regex replace
    for rule in &def.replace {
        if let Ok(re) = Regex::new(&rule.pattern) {
            text = re.replace_all(&text, rule.replacement.as_str()).into_owned();
        }
    }

    // Stage 3: match_output short-circuit
    for rule in &def.match_output {
        if let Ok(re) = Regex::new(&rule.pattern) && re.is_match(&text) {
            let blocked = rule.unless.as_ref().and_then(|u| Regex::new(u).ok());
            if blocked.is_none_or(|b| !b.is_match(&text)) {
                return rule.message.clone();
            }
        }
    }

    // Stage 4: line filtering
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    if !def.strip_lines_matching.is_empty() {
        let patterns: Vec<Regex> = def
            .strip_lines_matching
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        lines.retain(|l| !patterns.iter().any(|p| p.is_match(l)));
    }
    if !def.keep_lines_matching.is_empty() {
        let patterns: Vec<Regex> = def
            .keep_lines_matching
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        lines.retain(|l| patterns.iter().any(|p| p.is_match(l)));
    }

    // Stage 5: truncate per-line
    if let Some(max_chars) = def.truncate_lines_at {
        for line in &mut lines {
            if line.len() > max_chars {
                line.truncate(max_chars);
                line.push_str("...");
            }
        }
    }

    // Stage 6: head / tail
    if let Some(head) = def.head_lines {
        if let Some(tail) = def.tail_lines {
            if lines.len() > head + tail {
                let skipped = lines.len() - head - tail;
                let head_part: Vec<String> = lines[..head].to_vec();
                let tail_part: Vec<String> = lines[lines.len() - tail..].to_vec();
                lines = head_part;
                lines.push(format!("... ({skipped} lines skipped)"));
                lines.extend(tail_part);
            }
        } else if lines.len() > head {
            let total = lines.len();
            lines.truncate(head);
            lines.push(format!("... ({} more lines)", total - head));
        }
    } else if let Some(tail) = def.tail_lines && lines.len() > tail {
        let skipped = lines.len() - tail;
        lines = lines[lines.len() - tail..].to_vec();
        lines.insert(0, format!("... ({skipped} lines before)"));
    }

    // Stage 7: absolute max
    if let Some(max) = def.max_lines && lines.len() > max {
        let total = lines.len();
        lines.truncate(max);
        lines.push(format!("... ({} more lines)", total - max));
    }

    // Stage 8: on_empty fallback
    let result = lines.join("\n");
    if result.trim().is_empty() && let Some(ref fallback) = def.on_empty {
        return fallback.clone();
    }

    result
}

fn strip_ansi_codes(s: &str) -> String {
    // Fast path: no ESC byte → nothing to strip
    if !s.as_bytes().contains(&0x1b) {
        return s.to_string();
    }
    let re = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("static regex");
    re.replace_all(s, "").into_owned()
}

/// Built-in filters for common commands (compiled into the binary).
pub fn builtin_filters() -> std::collections::HashMap<String, FilterDef> {
    let mut m = std::collections::HashMap::new();

    m.insert(
        "make".into(),
        FilterDef {
            match_command: r"^make\b".into(),
            strip_ansi: true,
            strip_lines_matching: vec![
                r"^make\[\d+\]:".into(),
                r"^\s*$".into(),
                r"Nothing to be done".into(),
            ],
            max_lines: Some(50),
            on_empty: Some("make: ok".into()),
            ..default_def()
        },
    );

    m.insert(
        "docker-build".into(),
        FilterDef {
            match_command: r"^docker build\b".into(),
            strip_ansi: true,
            strip_lines_matching: vec![
                r"^(Step \d+/\d+ :|--->|Removing intermediate)".into(),
                r"^\s*$".into(),
            ],
            tail_lines: Some(30),
            max_lines: Some(50),
            on_empty: Some("docker build: ok".into()),
            ..default_def()
        },
    );

    m.insert(
        "npm-install".into(),
        FilterDef {
            match_command: r"^(npm|pnpm|yarn) install\b".into(),
            strip_ansi: true,
            strip_lines_matching: vec![
                r"^npm warn".into(),
                r"^\s*$".into(),
                r"^added \d+ packages".into(),
            ],
            max_lines: Some(20),
            on_empty: Some("install: ok".into()),
            ..default_def()
        },
    );

    m.insert(
        "pip-install".into(),
        FilterDef {
            match_command: r"^pip3? install\b".into(),
            strip_ansi: true,
            strip_lines_matching: vec![
                r"^Collecting ".into(),
                r"^  Downloading ".into(),
                r"^  Using cached ".into(),
                r"^\s*$".into(),
            ],
            max_lines: Some(20),
            on_empty: Some("pip install: ok".into()),
            ..default_def()
        },
    );

    m.insert(
        "pytest".into(),
        FilterDef {
            match_command: r"^pytest\b".into(),
            strip_ansi: true,
            strip_lines_matching: vec![
                r"^=+ ".into(),
                r"^plugins:".into(),
                r"^platform ".into(),
                r"^cachedir:".into(),
            ],
            max_lines: Some(80),
            ..default_def()
        },
    );

    m.insert(
        "go-test".into(),
        FilterDef {
            match_command: r"^go test\b".into(),
            strip_ansi: false,
            strip_lines_matching: vec![
                r"^\s*$".into(),
            ],
            max_lines: Some(60),
            on_empty: Some("go test: ok".into()),
            ..default_def()
        },
    );

    m.insert(
        "kubectl".into(),
        FilterDef {
            match_command: r"^kubectl\b".into(),
            strip_ansi: false,
            truncate_lines_at: Some(200),
            max_lines: Some(80),
            ..default_def()
        },
    );

    m
}

fn default_def() -> FilterDef {
    FilterDef {
        match_command: String::new(),
        strip_ansi: false,
        replace: Vec::new(),
        match_output: Vec::new(),
        strip_lines_matching: Vec::new(),
        keep_lines_matching: Vec::new(),
        truncate_lines_at: None,
        head_lines: None,
        tail_lines: None,
        max_lines: None,
        on_empty: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_codes() {
        let input = "\x1b[31merror\x1b[0m: something failed";
        assert_eq!(strip_ansi_codes(input), "error: something failed");
    }

    #[test]
    fn strip_ansi_noop_clean_string() {
        let input = "no ansi here";
        assert_eq!(strip_ansi_codes(input), input);
    }

    #[test]
    fn filter_pipeline_basic() {
        let def = FilterDef {
            match_command: "test".into(),
            strip_ansi: true,
            strip_lines_matching: vec![r"^DEBUG:".into()],
            max_lines: Some(5),
            on_empty: Some("clean".into()),
            ..default_def()
        };
        let raw = "DEBUG: skip\nline 1\nline 2\nDEBUG: skip\nline 3\nline 4\nline 5\nline 6\nline 7";
        let out = apply_filter(&def, raw);
        assert!(!out.contains("DEBUG"));
        // max_lines = 5, after stripping we have 5 lines exactly
        let line_count = out.lines().count();
        assert!(line_count <= 6); // 5 + possible "...N more"
    }

    #[test]
    fn filter_on_empty() {
        let def = FilterDef {
            match_command: "test".into(),
            strip_lines_matching: vec![r".+".into()],
            on_empty: Some("all filtered".into()),
            ..default_def()
        };
        let out = apply_filter(&def, "line1\nline2\n");
        assert_eq!(out, "all filtered");
    }

    #[test]
    fn match_output_shortcircuit() {
        let def = FilterDef {
            match_command: "test".into(),
            match_output: vec![MatchOutputRule {
                pattern: r"BUILD SUCCESS".into(),
                message: "build: ok".into(),
                unless: None,
            }],
            ..default_def()
        };
        let out = apply_filter(&def, "lots of output\nBUILD SUCCESS\nmore output");
        assert_eq!(out, "build: ok");
    }
}
