//! Filename-suffix heuristics — low-confidence tiebreakers (weight 0.2).
//!
//! These rules only look at the basename, never directory structure. They
//! are deliberately weaker than [`crate::arch::dictionary`] so the fusion
//! step can use them to break ties without overriding stronger signals.

use crate::arch::dictionary::LayerHint;
use regex::Regex;
use std::sync::OnceLock;

const NAMING_WEIGHT: f64 = 0.2;

struct CompiledRule {
    re: Regex,
    layer: &'static str,
}

fn rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let raw: &[(&str, &str)] = &[
            // API surface: Handler / Controller. Match common extensions.
            (
                r"(?i)(Handler|Controller)\.(go|ts|tsx|js|jsx|java|py|rs|kt|cs|cc|cpp)$",
                "api",
            ),
            // Domain: Service / Manager / Factory / Builder.
            (
                r"(?i)(Service|Manager|Factory|Builder)\.(go|ts|tsx|js|jsx|java|py|rs|kt|cs|cc|cpp)$",
                "domain",
            ),
            // Infrastructure: Repository / Repo / Store / Dao.
            (
                r"(?i)(Repository|Repo|Store|Dao)\.(go|ts|tsx|js|jsx|java|py|rs|kt|cs|cc|cpp)$",
                "infra",
            ),
            // Tests — strict suffix forms.
            (r"(?i)(_test\.go|_test\.py|\.test\.(ts|tsx|js|jsx)|Test\.java|\.Spec\.[a-z]+|Spec\.(ts|tsx|js|jsx|java|kt|cs|rs|py|go))$", "test"),
            // Tests — name contains Mock / Fake / Stub / Fixture as a word
            // boundary. The basename must start (or follow `._-`) with one of
            // the markers and be followed by either an uppercase letter
            // (CamelCase split), a separator, or a file extension.
            (
                r"(?i)(^|[._-])(Mock|Fake|Stub|Fixture)([._-]|[A-Z]|\.[a-z]+$)",
                "test",
            ),
            // Generated: protobuf-flavored suffixes.
            (
                r"(?i)(\.pb\.go|\.pb\.ts|_pb2\.py)$",
                "generated",
            ),
        ];
        raw.iter()
            .map(|(pat, layer)| CompiledRule {
                re: Regex::new(pat).expect("naming regex must compile"),
                layer,
            })
            .collect()
    })
}

/// Classify a relative path based on its filename pattern. Returns every
/// matching layer hint; empty when no pattern matches.
pub fn classify_naming(rel_path: &str) -> Vec<LayerHint> {
    let normalized = rel_path.replace('\\', "/");
    let basename = normalized.rsplit('/').next().unwrap_or(&normalized);
    let mut hints = Vec::new();
    for rule in rules() {
        if rule.re.is_match(basename) {
            hints.push(LayerHint::new(rule.layer, NAMING_WEIGHT));
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_layer(hints: &[LayerHint], layer: &str) -> bool {
        hints.iter().any(|h| h.layer == layer)
    }

    #[test]
    fn handler_suffix_classifies_as_api() {
        let hints = classify_naming("src/web/UserHandler.go");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn controller_suffix_classifies_as_api() {
        let hints = classify_naming("src/web/UserController.ts");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn service_suffix_classifies_as_domain() {
        let hints = classify_naming("src/svc/AccountService.java");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn manager_suffix_classifies_as_domain() {
        let hints = classify_naming("internal/SessionManager.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn repository_suffix_classifies_as_infra() {
        let hints = classify_naming("src/data/UserRepository.kt");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn dao_suffix_classifies_as_infra() {
        let hints = classify_naming("src/data/UserDao.java");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn underscore_test_go_classifies_as_test() {
        let hints = classify_naming("pkg/foo_test.go");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn dot_test_ts_classifies_as_test() {
        let hints = classify_naming("src/foo.test.ts");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn java_test_class_classifies_as_test() {
        let hints = classify_naming("src/test/java/UserServiceTest.java");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn mock_in_name_classifies_as_test() {
        let hints = classify_naming("src/data/MockRepo.go");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn factory_suffix_classifies_as_domain() {
        let hints = classify_naming("src/dom/WidgetFactory.cs");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn builder_suffix_classifies_as_domain() {
        let hints = classify_naming("src/dom/RequestBuilder.java");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn pb_go_classifies_as_generated() {
        let hints = classify_naming("rpc/api.pb.go");
        assert!(
            has_layer(&hints, "generated"),
            "expected generated, got {hints:?}"
        );
    }

    #[test]
    fn pb2_py_classifies_as_generated() {
        let hints = classify_naming("rpc/api_pb2.py");
        assert!(
            has_layer(&hints, "generated"),
            "expected generated, got {hints:?}"
        );
    }

    #[test]
    fn naming_weight_is_low() {
        let hints = classify_naming("src/web/UserHandler.go");
        for hint in &hints {
            assert!(
                (hint.weight - NAMING_WEIGHT).abs() < f64::EPSILON,
                "naming hints carry the low tiebreaker weight"
            );
        }
    }

    #[test]
    fn unrelated_filename_returns_empty() {
        let hints = classify_naming("src/main.rs");
        assert!(
            hints.is_empty(),
            "main.rs has no naming-pattern hint, got {hints:?}"
        );
    }

    #[test]
    fn windows_path_is_normalized() {
        let hints = classify_naming("src\\web\\UserHandler.go");
        assert!(
            has_layer(&hints, "api"),
            "expected api after normalization, got {hints:?}"
        );
    }
}
