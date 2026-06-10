//! Confidence-gate tests: low-confidence (ambiguous) resolutions must not
//! masquerade as solid callers in query outputs.

mod common;
use code_abyss::graph::GraphQuery;
use common::*;

/// Dup is defined in two packages → the call from caller/c.go resolves
/// ambiguously at confidence 0.5.
fn ambiguous_fixture() -> Fixture {
    index_fixture(&[
        ("a/d.go", "package a\n\nfunc Dup() int { return 1 }\n"),
        ("b/d.go", "package b\n\nfunc Dup() int { return 2 }\n"),
        (
            "caller/c.go",
            "package caller\n\nfunc C() int { return Dup() }\n",
        ),
    ])
}

#[test]
fn default_threshold_hides_ambiguous_callers() {
    let fx = ambiguous_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let callers = gq.find_callers("Dup", 20, 0.7).unwrap();
    assert!(callers.is_empty(), "{callers:?}");
}

#[test]
fn zero_threshold_reveals_ambiguous_callers() {
    let fx = ambiguous_fixture();
    let gq = GraphQuery::new(&fx.repo);
    let callers = gq.find_callers("Dup", 20, 0.0).unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].confidence, 0.5);
    assert_eq!(callers[0].symbol, "C");
}

#[test]
fn impact_notes_excluded_low_confidence_refs() {
    let fx = ambiguous_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let gated = gq.impact_analysis("Dup", 3, 0.7).unwrap();
    assert!(gated.direct_callers.is_empty());
    assert!(
        gated
            .risk_factors
            .iter()
            .any(|f| f.contains("low-confidence")),
        "{:?}",
        gated.risk_factors
    );

    let open = gq.impact_analysis("Dup", 3, 0.0).unwrap();
    assert_eq!(open.direct_callers.len(), 1);
    assert!(
        !open
            .risk_factors
            .iter()
            .any(|f| f.contains("low-confidence")),
        "{:?}",
        open.risk_factors
    );
}

#[test]
fn confident_callers_pass_the_default_gate() {
    let fx = index_fixture(&[(
        "app/a.go",
        "package app\n\nfunc Solid() int { return 1 }\n\nfunc User() int { return Solid() }\n",
    )]);
    let gq = GraphQuery::new(&fx.repo);
    let callers = gq.find_callers("Solid", 20, 0.7).unwrap();
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].confidence, 1.0);
}
