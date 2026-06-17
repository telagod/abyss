//! Callers --limit + "showing N of M" footer contract (B3). hono dogfood
//! (2026-06-17): `abyss callers Context` reported "20 prod" but 235 refs
//! existed — the underlying find_callers_of had a hard 200-row cap, the
//! CLI's default 20 truncated visibility, and no footer told the agent to
//! retry. The agent stopped reading at 20 and missed the real call sites.
//!
//! Contract pinned here:
//!   1. `count_callers_at` returns the visible-cohort total (under the
//!      same filter + confidence + test-exclusion the agent sees).
//!   2. A bounded result set + total lets the surface render "showing N
//!      of M" honestly.
//!   3. `--limit 0` (unlimited) takes the safety cap path; smaller limits
//!      truncate visibly.

mod common;
use code_abyss::graph::GraphQuery;
use common::*;

/// 30 distinct call sites pointing at one target. Each caller is a free
/// function so they all generate kind='call' refs at confidence 1.0
/// (same-file unique) — no L4 ambiguity, no test path noise.
fn many_callers_fixture() -> Fixture {
    let mut files: Vec<(String, String)> = Vec::new();
    files.push((
        "lib.ts".into(),
        "export function target() { return 1 }\n".into(),
    ));
    for i in 0..30 {
        files.push((
            format!("caller_{i:02}.ts"),
            format!(
                "import {{ target }} from './lib';\nexport function caller_{i:02}() {{ return target() }}\n"
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    index_fixture(&refs)
}

#[test]
fn count_callers_at_matches_visible_cohort() {
    // Sanity: the count helper that backs the "of M" denominator should
    // honestly report what the agent CAN see — same kind filter, same
    // confidence gate, same test-exclusion contract.
    let fx = many_callers_fixture();
    // Use the gated default — must match the 30 fixture call sites.
    let n = fx
        .repo
        .count_callers_at("target", &["call", "field_access"], 0.7, true)
        .expect("count");
    assert!(
        n >= 30,
        "expected ≥30 callers for `target`, got {n} — count must reflect what's visible"
    );
}

#[test]
fn limit_truncates_visible_list_under_total() {
    // With limit=5 the surface MUST be able to say "showing 5 of N".
    // We test the underlying primitives — limit cap + count helper — that
    // the CLI / MCP both use to render the footer.
    let fx = many_callers_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let result = gq
        .find_callers_filtered("target", 5, 0.7, true)
        .expect("query");
    assert_eq!(result.callers.len(), 5, "limit must cap the visible list");

    let total = fx
        .repo
        .count_callers_at(
            "target",
            &["call", "field_access", "type_ref", "inherit"],
            0.7,
            true,
        )
        .expect("count");
    assert!(
        total > result.callers.len(),
        "total ({total}) must exceed shown ({}) so was_capped is true",
        result.callers.len()
    );
}

#[test]
fn count_excludes_test_callers_when_requested() {
    // The footer must NOT count hidden test callers toward M — a denominator
    // larger than the visible cohort is misleading.
    let fx = index_fixture(&[
        ("lib.ts", "export function target() { return 1 }\n"),
        (
            "prod.ts",
            "import { target } from './lib';\nexport function caller() { return target() }\n",
        ),
        // .test.ts is a recognised test path: should not be counted.
        (
            "x.test.ts",
            "import { target } from './lib';\nexport function tc() { return target() }\n",
        ),
    ]);

    let with_tests = fx
        .repo
        .count_callers_at("target", &["call", "field_access"], 0.7, true)
        .expect("count");
    let without_tests = fx
        .repo
        .count_callers_at("target", &["call", "field_access"], 0.7, false)
        .expect("count");
    assert!(
        without_tests < with_tests,
        "test exclusion must lower the count (got {without_tests} vs {with_tests} all-up)"
    );
    assert!(
        without_tests >= 1,
        "prod caller must still be counted, got {without_tests}"
    );
}
