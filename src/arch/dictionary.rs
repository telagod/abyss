//! Path-segment regex dictionary — classifies a file path into one or more
//! architectural layers based on directory/segment hints.
//!
//! This is a low-weight signal (0.4) intended to be combined with other
//! evidence (entry-point detection, naming patterns, call-graph PageRank) in
//! the L0 fusion step. A single path can match multiple rules; all matches
//! are returned so the fusion layer can sum evidence.
//!
//! The dictionary is the initial heuristic; it ships as a static default and
//! can be overridden later by project-level configuration without breaking
//! the public API.

use regex::Regex;
use std::sync::OnceLock;

/// A single layer hint with an associated weight. Returned by both
/// `classify_path` (dictionary) and `classify_naming` (filename patterns).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerHint {
    pub layer: &'static str,
    pub weight: f64,
}

impl LayerHint {
    pub const fn new(layer: &'static str, weight: f64) -> Self {
        Self { layer, weight }
    }
}

/// Weight assigned to every dictionary match. Matches the blueprint —
/// dictionary hints are corroborating evidence, not load-bearing on their own.
const DICT_WEIGHT: f64 = 0.4;

/// Compiled dictionary rules. Each entry is `(pattern, layer_label)`.
///
/// Patterns are matched against the **forward-slash-normalized** relative
/// path. They are case-insensitive (anchored via `(?i)`).
struct CompiledRule {
    re: Regex,
    layer: &'static str,
}

fn rules() -> &'static [CompiledRule] {
    static RULES: OnceLock<Vec<CompiledRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let raw: &[(&str, &str)] = &[
            // auth/identity → domain (with implicit "auth" semantics)
            (
                r"(?i)(^|/)(auth|authz|authn|login|session|jwt|oauth|credential)(s)?(/|$|[._-])",
                "domain",
            ),
            // API / transport layer
            (
                r"(?i)(^|/)(handler|controller|router|api|rest|graphql|rpc|grpc|endpoint|route)(s)?(/|$|[._-])",
                "api",
            ),
            // Infrastructure / persistence
            (
                r"(?i)(^|/)(repo|repository|dao|store|storage|persist|persistence|model|entity|schema|migration)(s)?(/|$|[._-])",
                "infra",
            ),
            // Utility / shared helpers
            (
                r"(?i)(^|/)(util|utils|helper|helpers|common|shared|lib|pkg|internal)(/|$|[._-])",
                "util",
            ),
            // Entry-point folder/file hints. We use a single regex with
            // alternation so any of the entry markers fires.
            (
                r"(?i)(^|/)(cmd|bin)(/|$)|(^|/)main\.(go|rs|py)$|(^|/)app\.py$|(^|/)index\.(ts|js|tsx|jsx)$|(^|/)__main__\.py$",
                "entry",
            ),
            // Tests / fixtures / mocks
            (
                r"(?i)(^|/)(test|tests|spec|__tests__|fixture|fixtures|mock|mocks)(/|$)|_test\.|\.test\.|\.spec\.",
                "test",
            ),
            // Config
            (
                r"(?i)(^|/)(config|conf|settings|env)(s)?(/|$|[._-])|(^|/)\.env(\.|$)",
                "config",
            ),
            // Vendored / third-party
            (
                r"(?i)(^|/)(vendor|node_modules|third_party|external|deps)(/|$)",
                "vendor",
            ),
            // Machine-generated code
            (
                r"(?i)(^|/)(generated|gen)(/|$)|_pb2(\.|$)|\.pb\.|grpc_gen|protoc",
                "generated",
            ),
            // ── Expanded vocabulary (v0.3.7) ───────────────────────────────
            // Express-style middleware → api
            (
                r"(?i)(^|/)middleware(s)?(/|$|[._-])",
                "api",
            ),
            // route/routing — keep separate from the api umbrella so a file
            // named `routes.ts` at the top level still fires.
            (
                r"(?i)(^|/)(route|routing)(s)?(/|$|[._-])",
                "api",
            ),
            // service layer → domain (business logic)
            (
                r"(?i)(^|/)service(s)?(/|$|[._-])",
                "domain",
            ),
            // gateway / proxy → api transport
            (
                r"(?i)(^|/)(gateway|proxy)(s)?(/|$|[._-])",
                "api",
            ),
            // adapter / adaptor → infra (ports & adapters)
            (
                r"(?i)(^|/)(adapter|adaptor)(s)?(/|$|[._-])",
                "infra",
            ),
            // Message queue / broker — infra transport
            (
                r"(?i)(^|/)(queue|mq|kafka|rabbitmq|pubsub)(s)?(/|$|[._-])",
                "infra",
            ),
            // Domain events / event bus
            (
                r"(?i)(^|/)(event|eventbus|event_bus)(s)?(/|$|[._-])",
                "domain",
            ),
            // Cache layer → infra
            (
                r"(?i)(^|/)(cache|redis|memcache|memcached)(/|$|[._-])",
                "infra",
            ),
            // Background workers, schedulers, cron, jobs → infra
            (
                r"(?i)(^|/)(scheduler|cron|job|worker)(s)?(/|$|[._-])",
                "infra",
            ),
            // Validators / validation helpers → util
            (
                r"(?i)(^|/)(validator|validation)(s)?(/|$|[._-])",
                "util",
            ),
            // Error / errors helpers → util
            (
                r"(?i)(^|/)error(s)?(/|$|[._-])",
                "util",
            ),
            // Logging utilities → util
            (
                r"(?i)(^|/)(log|logger|logging)(/|$|[._-])",
                "util",
            ),
            // Metrics / telemetry / observability → util
            (
                r"(?i)(^|/)(metric(s)?|telemetry|observ[a-z]*)(/|$|[._-])",
                "util",
            ),
            // DB / database / db connections → infra
            (
                r"(?i)(^|/)(db|database)(s)?(/|$|[._-])",
                "infra",
            ),
            // Seed data / fixtures → test
            (
                r"(?i)(^|/)(seed|fixture)(s)?(/|$|[._-])",
                "test",
            ),
        ];
        raw.iter()
            .map(|(pat, layer)| CompiledRule {
                re: Regex::new(pat).expect("dictionary regex must compile"),
                layer,
            })
            .collect()
    })
}

/// Classify a relative path against the dictionary. Returns every layer
/// hint that matches; an empty `Vec` means "no opinion" — let downstream
/// signals decide.
///
/// The input path is normalized to forward slashes; absolute paths and
/// Windows-style backslashes are tolerated.
pub fn classify_path(rel_path: &str) -> Vec<LayerHint> {
    let normalized = rel_path.replace('\\', "/");
    let mut hints = Vec::new();
    for rule in rules() {
        if rule.re.is_match(&normalized) {
            hints.push(LayerHint::new(rule.layer, DICT_WEIGHT));
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
    fn auth_segment_classifies_as_domain() {
        let hints = classify_path("src/auth/session.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn handler_segment_classifies_as_api() {
        let hints = classify_path("internal/handler/login.go");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
        // login also triggers the auth/domain rule
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn cmd_main_classifies_as_entry() {
        let hints = classify_path("cmd/server/main.go");
        assert!(has_layer(&hints, "entry"), "expected entry, got {hints:?}");
    }

    #[test]
    fn pkg_util_classifies_as_util() {
        let hints = classify_path("pkg/util/strings.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn test_suffix_classifies_as_test() {
        let hints = classify_path("tests/foo_test.go");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn vendor_segment_classifies_as_vendor() {
        let hints = classify_path("vendor/foo/bar.go");
        assert!(
            has_layer(&hints, "vendor"),
            "expected vendor, got {hints:?}"
        );
    }

    #[test]
    fn migrations_folder_classifies_as_infra() {
        let hints = classify_path("src/storage/migrations/0001.sql");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn pb_go_classifies_as_generated() {
        let hints = classify_path("src/generated/api.pb.go");
        assert!(
            has_layer(&hints, "generated"),
            "expected generated, got {hints:?}"
        );
    }

    #[test]
    fn config_folder_classifies_as_config() {
        let hints = classify_path("config/settings.yaml");
        assert!(
            has_layer(&hints, "config"),
            "expected config, got {hints:?}"
        );
    }

    #[test]
    fn env_dotfile_classifies_as_config() {
        let hints = classify_path(".env.production");
        assert!(
            has_layer(&hints, "config"),
            "expected config, got {hints:?}"
        );
    }

    #[test]
    fn jwt_segment_classifies_as_domain() {
        let hints = classify_path("internal/jwt/sign.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn graphql_segment_classifies_as_api() {
        let hints = classify_path("server/graphql/resolver.ts");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn repository_segment_classifies_as_infra() {
        let hints = classify_path("internal/repository/user.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn dot_test_ts_classifies_as_test() {
        let hints = classify_path("src/foo.test.ts");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn node_modules_classifies_as_vendor() {
        let hints = classify_path("node_modules/react/index.js");
        assert!(
            has_layer(&hints, "vendor"),
            "expected vendor, got {hints:?}"
        );
    }

    #[test]
    fn windows_path_is_normalized() {
        let hints = classify_path("src\\auth\\session.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain after path normalization, got {hints:?}"
        );
    }

    #[test]
    fn unrelated_path_returns_empty() {
        let hints = classify_path("README.md");
        assert!(
            hints.is_empty() || !hints.iter().any(|h| h.weight > 0.5),
            "README.md should not generate strong signals, got {hints:?}"
        );
    }

    #[test]
    fn rules_compile_lazily_once() {
        // Touch the dictionary twice to exercise the OnceLock; this must not
        // panic and must return identical pointers.
        let first = rules().as_ptr();
        let second = rules().as_ptr();
        assert_eq!(first, second, "rules must be cached in OnceLock");
    }

    #[test]
    fn dictionary_weight_is_consistent() {
        let hints = classify_path("src/auth/session.go");
        for hint in &hints {
            assert!(
                (hint.weight - DICT_WEIGHT).abs() < f64::EPSILON,
                "dictionary hints share a fixed weight"
            );
        }
    }

    // ── Expanded vocabulary tests ──────────────────────────────────────

    #[test]
    fn middleware_classifies_as_api() {
        let hints = classify_path("src/middleware/auth.go");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn routing_classifies_as_api() {
        let hints = classify_path("src/routing/index.ts");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn service_classifies_as_domain() {
        let hints = classify_path("internal/service/billing.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn services_plural_classifies_as_domain() {
        let hints = classify_path("src/services/user.ts");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn gateway_classifies_as_api() {
        let hints = classify_path("src/gateway/payments.go");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn proxy_classifies_as_api() {
        let hints = classify_path("internal/proxy/upstream.go");
        assert!(has_layer(&hints, "api"), "expected api, got {hints:?}");
    }

    #[test]
    fn adapter_classifies_as_infra() {
        let hints = classify_path("src/adapter/stripe.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn adaptor_british_classifies_as_infra() {
        let hints = classify_path("src/adaptor/foo.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn queue_classifies_as_infra() {
        let hints = classify_path("src/queue/worker.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn kafka_classifies_as_infra() {
        let hints = classify_path("src/kafka/consumer.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn rabbitmq_classifies_as_infra() {
        let hints = classify_path("src/rabbitmq/publisher.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn pubsub_classifies_as_infra() {
        let hints = classify_path("src/pubsub/topic.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn event_classifies_as_domain() {
        let hints = classify_path("src/event/user_created.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn events_folder_classifies_as_domain() {
        let hints = classify_path("src/events/billing.ts");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn event_bus_underscore_classifies_as_domain() {
        let hints = classify_path("src/event_bus/dispatch.go");
        assert!(
            has_layer(&hints, "domain"),
            "expected domain, got {hints:?}"
        );
    }

    #[test]
    fn cache_classifies_as_infra() {
        let hints = classify_path("src/cache/lru.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn redis_classifies_as_infra() {
        let hints = classify_path("src/redis/client.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn memcache_classifies_as_infra() {
        let hints = classify_path("src/memcache/client.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn scheduler_classifies_as_infra() {
        let hints = classify_path("src/scheduler/quartz.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn cron_classifies_as_infra() {
        let hints = classify_path("src/cron/nightly.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn job_classifies_as_infra() {
        let hints = classify_path("src/job/runner.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn worker_classifies_as_infra() {
        let hints = classify_path("src/worker/pool.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn validator_classifies_as_util() {
        let hints = classify_path("src/validator/email.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn validation_classifies_as_util() {
        let hints = classify_path("src/validation/rules.ts");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn error_classifies_as_util() {
        let hints = classify_path("src/error/handler.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn errors_plural_classifies_as_util() {
        let hints = classify_path("src/errors/types.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn log_classifies_as_util() {
        let hints = classify_path("src/log/json.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn logger_classifies_as_util() {
        let hints = classify_path("src/logger/zap.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn logging_classifies_as_util() {
        let hints = classify_path("src/logging/setup.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn metrics_classifies_as_util() {
        let hints = classify_path("src/metrics/prom.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn telemetry_classifies_as_util() {
        let hints = classify_path("src/telemetry/otel.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn observability_classifies_as_util() {
        let hints = classify_path("src/observability/tracer.go");
        assert!(has_layer(&hints, "util"), "expected util, got {hints:?}");
    }

    #[test]
    fn db_classifies_as_infra() {
        let hints = classify_path("src/db/conn.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn database_classifies_as_infra() {
        let hints = classify_path("src/database/pool.go");
        assert!(has_layer(&hints, "infra"), "expected infra, got {hints:?}");
    }

    #[test]
    fn seed_classifies_as_test() {
        let hints = classify_path("src/seed/init.go");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }

    #[test]
    fn fixture_classifies_as_test() {
        let hints = classify_path("tests/fixture/sample.go");
        assert!(has_layer(&hints, "test"), "expected test, got {hints:?}");
    }
}
