//! User-level overrides for the layer dictionary.
//!
//! Loaded from `<workspace>/.code-abyss/arch.toml` at the start of the
//! arch inference pass. The TOML schema is intentionally tiny:
//!
//! ```toml
//! [layers]
//! # additional path-segment rules, layered ON TOP of the defaults
//! graph    = { layer = "infra", weight = 0.6 }   # weight optional, defaults 0.5
//! temporal = { layer = "infra", weight = 0.6 }
//! indexer  = { layer = "infra", weight = 0.6 }
//!
//! [ignore]
//! # regex patterns that, if matched, skip arch inference entirely for that file
//! patterns = ["^vendor/", "^node_modules/"]
//! ```
//!
//! Rules are matched as case-insensitive path-segment regexes
//! (`(?i)(^|/)<key>(/|$|[._-])`) so the user can write `graph` and have it
//! catch `src/graph/foo.go`, `graph.go`, etc. without having to author a
//! full regex.
//!
//! Behaviour on malformed TOML: log a warning and return `None`. The arch
//! pipeline must never crash because of a user config typo.
//!
//! See `docs/ARCH-LAYERS.md` for the user-facing documentation.
//!
//! Public surface is the [`load_overrides`] function in `src/arch/mod.rs`
//! and the two structs ([`ArchOverride`], [`LayerOverride`]).

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::dictionary::LayerHint;

/// Default weight assigned to an override rule when the user omits one.
/// Matches the "louder than a default dict hint (0.4) by default" intent —
/// users overriding the dictionary almost always mean to win the tie.
const DEFAULT_OVERRIDE_WEIGHT: f64 = 0.5;

/// Parsed contents of `.code-abyss/arch.toml`. Compiled regexes are kept
/// alongside the raw config so we only pay the regex compile cost once per
/// index run.
#[derive(Debug, Clone)]
pub struct ArchOverride {
    /// Compiled layer rules. Empty if `[layers]` is absent or empty.
    layer_rules: Vec<CompiledOverride>,
    /// Compiled ignore patterns. Empty if `[ignore].patterns` is absent.
    ignore_rules: Vec<Regex>,
}

impl ArchOverride {
    /// Number of layer rules + ignore patterns successfully compiled. Used
    /// for the info! log line and for tests.
    pub fn rule_count(&self) -> usize {
        self.layer_rules.len() + self.ignore_rules.len()
    }

    /// Number of layer rules only (useful for the info! breakdown).
    pub fn layer_rule_count(&self) -> usize {
        self.layer_rules.len()
    }

    /// Number of ignore rules only.
    pub fn ignore_rule_count(&self) -> usize {
        self.ignore_rules.len()
    }

    /// Apply user layer overrides to a path, returning the additional
    /// `LayerHint`s to append to the dictionary signal.
    pub fn classify_path(&self, rel_path: &str) -> Vec<LayerHint> {
        let normalized = rel_path.replace('\\', "/");
        let mut hints = Vec::new();
        for rule in &self.layer_rules {
            if rule.re.is_match(&normalized) {
                hints.push(LayerHint {
                    layer: rule.layer,
                    weight: rule.weight,
                });
            }
        }
        hints
    }

    /// True if any `[ignore].patterns` regex matches the path. Caller skips
    /// arch inference entirely for these files.
    pub fn is_ignored(&self, rel_path: &str) -> bool {
        let normalized = rel_path.replace('\\', "/");
        self.ignore_rules.iter().any(|r| r.is_match(&normalized))
    }

    /// Construct directly from a parsed TOML config. Public for tests.
    pub fn from_raw(raw: RawArchConfig) -> Self {
        let mut layer_rules: Vec<CompiledOverride> = Vec::new();
        for (key, rule) in raw.layers {
            // Skip rules with empty key or empty layer label — user typo.
            if key.is_empty() || rule.layer.is_empty() {
                continue;
            }
            let weight = rule.weight.unwrap_or(DEFAULT_OVERRIDE_WEIGHT);
            // Quote the key so regex metachars in segment names (e.g. dots)
            // are treated literally. We anchor the segment to a path boundary
            // so `graph` doesn't accidentally match `paragraph_writer.go`.
            let pattern = format!("(?i)(^|/){}(/|$|[._-])", regex::escape(&key));
            match Regex::new(&pattern) {
                Ok(re) => {
                    // Leak the layer label so it satisfies the &'static str
                    // contract that LayerHint uses elsewhere. Bounded — runs
                    // once per indexer pass, at most a few dozen entries.
                    let leaked: &'static str = Box::leak(rule.layer.into_boxed_str());
                    layer_rules.push(CompiledOverride {
                        re,
                        layer: leaked,
                        weight,
                    });
                }
                Err(e) => {
                    tracing::warn!("arch.toml: skipping malformed layer key {key:?}: {e}");
                }
            }
        }

        let mut ignore_rules: Vec<Regex> = Vec::new();
        for pat in raw.ignore.patterns {
            match Regex::new(&pat) {
                Ok(re) => ignore_rules.push(re),
                Err(e) => {
                    tracing::warn!("arch.toml: skipping malformed ignore pattern {pat:?}: {e}");
                }
            }
        }

        ArchOverride {
            layer_rules,
            ignore_rules,
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledOverride {
    re: Regex,
    layer: &'static str,
    weight: f64,
}

/// Raw TOML structure as deserialized by serde. Kept public so callers can
/// hand-construct configs for testing without touching disk.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RawArchConfig {
    #[serde(default)]
    pub layers: HashMap<String, LayerOverride>,
    #[serde(default)]
    pub ignore: IgnoreSection,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LayerOverride {
    pub layer: String,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IgnoreSection {
    #[serde(default)]
    pub patterns: Vec<String>,
}

/// Load `<workspace>/.code-abyss/arch.toml`. Returns `None` silently if the
/// file is missing or malformed — the arch pipeline runs with defaults.
/// Emits a single `info!` line on successful load.
pub fn load_overrides(workspace: &Path) -> Option<ArchOverride> {
    let path = workspace.join(".code-abyss").join("arch.toml");
    let bytes = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("arch.toml: failed to read {}: {}", path.display(), e);
            return None;
        }
    };
    let raw: RawArchConfig = match toml::from_str(&bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("arch.toml: malformed TOML at {}: {}", path.display(), e);
            return None;
        }
    };
    let overrides = ArchOverride::from_raw(raw);
    tracing::info!(
        "arch.toml loaded: {} layer rules, {} ignore rules from {}",
        overrides.layer_rule_count(),
        overrides.ignore_rule_count(),
        path.display()
    );
    Some(overrides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_arch_toml(dir: &Path, body: &str) {
        let cfg_dir = dir.join(".code-abyss");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let mut f = std::fs::File::create(cfg_dir.join("arch.toml")).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        assert!(load_overrides(tmp.path()).is_none());
    }

    #[test]
    fn malformed_toml_returns_none() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(tmp.path(), "this is = not valid TOML [[");
        assert!(
            load_overrides(tmp.path()).is_none(),
            "malformed TOML must be tolerated, not panic"
        );
    }

    #[test]
    fn loads_layer_rules_with_default_weight() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
graph = { layer = "infra" }
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert_eq!(overrides.layer_rule_count(), 1);
        let hints = overrides.classify_path("src/graph/languages/go.rs");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].layer, "infra");
        assert!((hints[0].weight - DEFAULT_OVERRIDE_WEIGHT).abs() < f64::EPSILON);
    }

    #[test]
    fn loads_layer_rules_with_explicit_weight() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
graph    = { layer = "infra", weight = 0.6 }
temporal = { layer = "infra", weight = 0.7 }
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert_eq!(overrides.layer_rule_count(), 2);

        let g = overrides.classify_path("src/graph/foo.rs");
        assert_eq!(g.len(), 1);
        assert!((g[0].weight - 0.6).abs() < f64::EPSILON);

        let t = overrides.classify_path("src/temporal/git_parser.rs");
        assert_eq!(t.len(), 1);
        assert!((t[0].weight - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn ignore_pattern_matches() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[ignore]
patterns = ["^vendor/", "^node_modules/"]
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert!(overrides.is_ignored("vendor/foo/bar.go"));
        assert!(overrides.is_ignored("node_modules/react/index.js"));
        assert!(!overrides.is_ignored("src/main.go"));
    }

    #[test]
    fn override_anchors_to_path_segment_not_substring() {
        // "graph" should not match "paragraph_writer.go" — the rule anchors
        // to a path boundary on both sides.
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
graph = { layer = "infra" }
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        let hints = overrides.classify_path("src/paragraph_writer.go");
        assert!(
            hints.is_empty(),
            "substring match should not fire, got {hints:?}"
        );
    }

    #[test]
    fn malformed_layer_rule_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        // Empty key + valid key in same file — only the valid one should
        // survive, and loading must not return None.
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
"" = { layer = "infra" }
graph = { layer = "infra" }
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert_eq!(
            overrides.layer_rule_count(),
            1,
            "empty-key rule should be dropped"
        );
    }

    #[test]
    fn malformed_ignore_regex_is_skipped() {
        let tmp = TempDir::new().unwrap();
        // `[unclosed` is invalid regex syntax — must be skipped, not crash.
        write_arch_toml(
            tmp.path(),
            r#"
[ignore]
patterns = ["[unclosed", "^vendor/"]
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert_eq!(overrides.ignore_rule_count(), 1);
        assert!(overrides.is_ignored("vendor/x.go"));
    }

    #[test]
    fn windows_path_is_normalized_in_override() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
graph = { layer = "infra" }
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        let hints = overrides.classify_path("src\\graph\\foo.rs");
        assert_eq!(hints.len(), 1, "backslash path must be normalized");
    }

    #[test]
    fn rule_count_sums_layers_and_ignores() {
        let tmp = TempDir::new().unwrap();
        write_arch_toml(
            tmp.path(),
            r#"
[layers]
a = { layer = "infra" }
b = { layer = "domain" }

[ignore]
patterns = ["^vendor/"]
"#,
        );
        let overrides = load_overrides(tmp.path()).expect("config should load");
        assert_eq!(overrides.layer_rule_count(), 2);
        assert_eq!(overrides.ignore_rule_count(), 1);
        assert_eq!(overrides.rule_count(), 3);
    }
}
