//! Callers test-exclusion contract: agents on unfamiliar codebases want prod
//! call sites first. By default `find_callers_filtered` drops test callers
//! and reports the dropped count so the surface can hint at the
//! `--include-tests` retry. `include_tests=true` restores the full graph.

mod common;
use code_abyss::graph::GraphQuery;
use common::*;

/// Two prod callers (`ProdA`, `ProdB`) and three test callers across the
/// common test-path conventions (`_test.go`, `.test.ts`, and an in-tree
/// `tests/` directory) so we exercise every branch of
/// `Repository::is_test_file`.
fn mixed_fixture() -> Fixture {
    index_fixture(&[
        (
            "app/target.go",
            "package app\n\nfunc Target() int { return 1 }\n",
        ),
        (
            "app/prod_a.go",
            "package app\n\nfunc ProdA() int { return Target() }\n",
        ),
        (
            "app/prod_b.go",
            "package app\n\nfunc ProdB() int { return Target() }\n",
        ),
        // Test callers — three distinct path shapes
        (
            "app/target_test.go",
            "package app\n\nfunc TestTargetGo() int { return Target() }\n",
        ),
        (
            "app/target.test.go",
            "package app\n\nfunc TestTargetDotted() int { return Target() }\n",
        ),
        (
            "tests/integration_test.go",
            "package tests\n\nimport \"x\"\n\nfunc TestTargetDir() int { return x.Target() }\n",
        ),
    ])
}

#[test]
fn default_excludes_tests_and_counts_them() {
    let fx = mixed_fixture();
    let gq = GraphQuery::new(&fx.repo);

    // Default agent-facing call: include_tests=false. Surface must hide all
    // three test callers and report the dropped count so the CLI/MCP layer
    // can render the "use --include-tests to see all" hint.
    let result = gq.find_callers_filtered("Target", 20, 0.0, false).unwrap();

    assert_eq!(
        result.callers.len(),
        2,
        "expected 2 prod callers, got {:?}",
        result.callers
    );
    assert!(
        result.callers.iter().all(|c| !c.is_test),
        "test caller leaked into default output: {:?}",
        result.callers
    );

    let names: Vec<&str> = result.callers.iter().map(|c| c.symbol.as_str()).collect();
    assert!(names.contains(&"ProdA"), "{names:?}");
    assert!(names.contains(&"ProdB"), "{names:?}");

    assert_eq!(
        result.excluded_tests, 3,
        "expected 3 test callers excluded, got {} ({:?})",
        result.excluded_tests, result.callers
    );
}

#[test]
fn include_tests_returns_full_caller_set() {
    let fx = mixed_fixture();
    let gq = GraphQuery::new(&fx.repo);

    // Explicit opt-in restores the legacy behaviour: every caller surfaces
    // and the excluded count is zero (nothing was filtered).
    let result = gq.find_callers_filtered("Target", 20, 0.0, true).unwrap();

    assert_eq!(
        result.callers.len(),
        5,
        "expected all 5 callers when include_tests=true, got {:?}",
        result.callers
    );
    assert_eq!(result.excluded_tests, 0);

    let test_count = result.callers.iter().filter(|c| c.is_test).count();
    assert_eq!(test_count, 3, "{:?}", result.callers);
}

#[test]
fn legacy_find_callers_preserves_old_behaviour() {
    // The struct-free entry point is still load-bearing for existing tests
    // and library consumers — it must keep returning the full caller list so
    // back-compat callers don't silently lose their test results when they
    // upgrade.
    let fx = mixed_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let callers = gq.find_callers("Target", 20, 0.0).unwrap();
    assert_eq!(callers.len(), 5, "{callers:?}");
    assert!(callers.iter().any(|c| c.is_test));
}

#[test]
fn empty_when_nothing_calls_target() {
    let fx = mixed_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered("DoesNotExist", 20, 0.0, false)
        .unwrap();
    assert!(result.callers.is_empty());
    assert_eq!(result.excluded_tests, 0);
}
