//! End-to-end graph query tests: index a fixture workspace, then exercise
//! find_callers / impact_analysis the way the CLI and MCP server do.

mod common;
use code_abyss::graph::GraphQuery;
use common::*;

/// Call chain: Target ← DirectA (same file), Target ← DirectB (same pkg),
/// DirectB ← Indirect, Target ← TestTarget (test file).
fn chain_fixture() -> Fixture {
    index_fixture(&[
        (
            "app/a.go",
            "package app\n\nfunc Target() int { return 1 }\n\nfunc DirectA() int { return Target() }\n",
        ),
        (
            "app/b.go",
            "package app\n\nfunc DirectB() int { return Target() }\n",
        ),
        (
            "app/c.go",
            "package app\n\nfunc Indirect() int { return DirectB() }\n",
        ),
        (
            "app/a_test.go",
            "package app\n\nfunc TestTarget() int { return Target() }\n",
        ),
    ])
}

#[test]
fn find_callers_returns_all_with_confidence_order() {
    let fx = chain_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let callers = gq.find_callers("Target", 20, 0.0).unwrap();
    assert_eq!(callers.len(), 3, "{callers:?}");
    // Ordered by confidence descending: same-file first.
    assert_eq!(callers[0].symbol, "DirectA");
    assert_eq!(callers[0].confidence, 1.0);
    let names: Vec<&str> = callers.iter().map(|c| c.symbol.as_str()).collect();
    assert!(names.contains(&"DirectB"));
    assert!(names.contains(&"TestTarget"));
    let test_caller = callers.iter().find(|c| c.symbol == "TestTarget").unwrap();
    assert!(test_caller.is_test);
}

#[test]
fn impact_analysis_walks_transitive_and_tests() {
    let fx = chain_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let impact = gq.impact_analysis("Target", 3, 0.7).unwrap();

    let direct: Vec<&str> = impact
        .direct_callers
        .iter()
        .map(|c| c.symbol.as_str())
        .collect();
    assert!(direct.contains(&"DirectA"), "{direct:?}");
    assert!(direct.contains(&"DirectB"));

    let transitive: Vec<&str> = impact
        .transitive_callers
        .iter()
        .map(|c| c.symbol.as_str())
        .collect();
    assert!(transitive.contains(&"Indirect"), "{transitive:?}");

    assert_eq!(impact.affected_tests.len(), 1);
    assert_eq!(impact.affected_tests[0].symbol, "TestTarget");

    // DirectA/DirectB/Indirect are not covered by the only test symbol.
    assert!(!impact.uncovered_paths.is_empty());
    assert!(impact.risk_score > 0.0);
    assert!(impact.risk_score <= 10.0);
}

#[test]
fn impact_of_unknown_symbol_is_empty_and_low_risk() {
    let fx = chain_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let impact = gq.impact_analysis("DoesNotExist", 3, 0.7).unwrap();
    assert!(impact.direct_callers.is_empty());
    assert!(impact.transitive_callers.is_empty());
    assert_eq!(impact.risk_score, 0.0);
}

#[test]
fn symbol_and_fulltext_search_find_indexed_code() {
    let fx = chain_fixture();
    let symbols = code_abyss::search::symbol::search(&fx.repo, "Target", 10).unwrap();
    assert!(!symbols.is_empty());
    assert!(symbols.iter().any(|s| s.name == "Target"));

    let engine = code_abyss::search::SearchEngine::new(&fx.repo, None);
    let results = engine.search("Target", 10).unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.file_path.contains("a.go")));
}

#[test]
fn reindex_is_idempotent() {
    let fx = chain_fixture();
    let pipeline = code_abyss::indexer::IndexPipeline::new(fx.config.clone());
    let before_refs = fx.repo.ref_count().unwrap();
    let before_symbols = fx.repo.symbol_count().unwrap();

    let stats = pipeline.run_structural(&fx.repo).unwrap();
    assert_eq!(stats.indexed, 0, "unchanged files must not reindex");
    assert_eq!(fx.repo.ref_count().unwrap(), before_refs);
    assert_eq!(fx.repo.symbol_count().unwrap(), before_symbols);
}

#[test]
fn deleted_files_are_removed_from_index() {
    let fx = index_fixture(&[
        ("a.go", "package main\n\nfunc A() int { return 1 }\n"),
        ("b.go", "package main\n\nfunc B() int { return A() }\n"),
    ]);
    assert_eq!(fx.repo.file_count().unwrap(), 2);

    std::fs::remove_file(fx.config.workspace.join("b.go")).unwrap();
    let pipeline = code_abyss::indexer::IndexPipeline::new(fx.config.clone());
    let stats = pipeline.run_structural(&fx.repo).unwrap();
    assert_eq!(stats.deleted, 1);
    assert_eq!(fx.repo.file_count().unwrap(), 1);
    // Refs from the deleted file must be gone too.
    assert_eq!(call_refs_to(&fx.repo, "A").len(), 0);
}
