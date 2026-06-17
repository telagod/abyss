//! Callers kind contract for `inherit` edges. Django dogfood (2026-06-17):
//! `abyss callers Model` returned 7 instead of the ~983 expected because the
//! default kind set was `[call, field_access, type_ref]` — `inherit` refs
//! emitted by the Python extractor (one per `class Sub(Base):`) were
//! silently dropped. Same blindness pattern as v0.5.0 G1, different label.
//!
//! These tests pin three contracts:
//!   1. Default callers include kind='inherit'.
//!   2. `--inherits-only` (CallerKindFilter::InheritsOnly) surfaces only
//!      inheritance users — useful for "every subclass of Base".
//!   3. The legacy filters (CallsOnly, TypesOnly) still exclude inheritance
//!      edges, so an agent that explicitly asked for invocation users
//!      doesn't get drowned in subclasses.

mod common;
use code_abyss::graph::{CallerKindFilter, GraphQuery};
use common::*;

/// Mini Django-shape fixture: one Base, three Subs, plus one invocation user
/// of an unrelated function so the fixture has a mix of edge kinds.
fn base_class_fixture() -> Fixture {
    index_fixture(&[
        (
            "base.py",
            "class Base:\n    def foo(self):\n        return 1\n\n\
             def helper():\n    return 2\n",
        ),
        (
            "sub_a.py",
            "from base import Base\n\nclass SubA(Base):\n    pass\n",
        ),
        (
            "sub_b.py",
            "from base import Base\n\nclass SubB(Base):\n    pass\n",
        ),
        (
            "sub_c.py",
            "from base import Base\n\nclass SubC(Base):\n    pass\n",
        ),
        (
            "calls.py",
            "from base import helper\n\ndef use_it():\n    return helper()\n",
        ),
    ])
}

#[test]
fn default_callers_include_inherit_edges() {
    // Pre-B1 bug: default filter dropped kind='inherit' rows. `callers Base`
    // returned the same 0 (no calls of `Base()`) instead of the three
    // subclass files. Mirrors Django Model's headline failure.
    let fx = base_class_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered("Base", 20, 0.0, true)
        .expect("query");
    let paths: Vec<&str> = result
        .callers
        .iter()
        .map(|c| c.file_path.as_str())
        .collect();
    let kinds: Vec<&str> = result.callers.iter().map(|c| c.kind.as_str()).collect();
    assert!(
        result.callers.len() >= 3,
        "expected ≥3 callers for Base (three subclasses inheriting), got {} ({paths:?} / {kinds:?})",
        result.callers.len(),
    );
    assert!(
        paths.contains(&"sub_a.py") && paths.contains(&"sub_b.py") && paths.contains(&"sub_c.py"),
        "default callers must include all three subclass files, got {paths:?}",
    );
    assert!(
        kinds.contains(&"inherit"),
        "at least one default caller must carry kind=inherit, got {kinds:?}",
    );
}

#[test]
fn inherits_only_filter_returns_only_inheritance_users() {
    // `--inherits-only` is the dual of `--types-only`: only inheritance
    // edges survive. Calls of `helper()` must disappear.
    let fx = base_class_fixture();
    let gq = GraphQuery::new(&fx.repo);

    // Base — three inheritance users, no invocations: must return three.
    let result = gq
        .find_callers_filtered_kinds("Base", 20, 0.0, true, CallerKindFilter::InheritsOnly)
        .expect("query");
    assert_eq!(
        result.callers.len(),
        3,
        "inherits-only on Base must return three subclasses, got {:?}",
        result.callers,
    );
    assert!(
        result.callers.iter().all(|c| c.kind == "inherit"),
        "all rows must be kind=inherit, got {:?}",
        result.callers,
    );

    // helper — one invocation user, no inheritance users: must return zero.
    let result = gq
        .find_callers_filtered_kinds("helper", 20, 0.0, true, CallerKindFilter::InheritsOnly)
        .expect("query");
    assert_eq!(
        result.callers.len(),
        0,
        "inherits-only on helper (no subclasses) must return zero, got {:?}",
        result.callers,
    );
}

#[test]
fn calls_only_filter_still_excludes_inherit_edges() {
    // Symmetric guard: an agent that asked for `--calls-only` on a base
    // class must not get the subclass list. This is what the original
    // v0.5.0 filter did right; the regression would be if `inherit` leaked
    // into `CallsOnly`.
    let fx = base_class_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered_kinds("Base", 20, 0.0, true, CallerKindFilter::CallsOnly)
        .expect("query");
    assert!(
        result.callers.iter().all(|c| c.kind != "inherit"),
        "calls-only must not surface kind=inherit rows, got {:?}",
        result.callers,
    );
}

#[test]
fn types_only_filter_does_not_include_inherit() {
    // `--types-only` is for type-position users (annotations, generics,
    // extends-in-TS). Python `inherit` refs are a separate axis; they must
    // stay out of types-only too.
    let fx = base_class_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered_kinds("Base", 20, 0.0, true, CallerKindFilter::TypesOnly)
        .expect("query");
    assert!(
        result.callers.iter().all(|c| c.kind == "type_ref"),
        "types-only rows must all be kind=type_ref, got {:?}",
        result.callers,
    );
}
