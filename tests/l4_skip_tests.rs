//! L4 + L4b candidate filter: when resolving an unqualified call by global
//! uniqueness, the resolver MUST skip symbols that live in test paths
//! (`__tests__`, `_test`, `.test`, `playground/`, …). On vite the
//! pre-fix path mis-bound an exported `debug` import to a same-named local
//! `function debug` in `playground/hmr-ssr/__tests__/hmr-ssr.spec.ts` —
//! the test fixture was the unique global match and the import resolved
//! there, polluting agent context.

mod common;
use common::*;

#[test]
fn l4_global_unique_picks_real_impl_over_test_fixture() {
    // Three files:
    //   * caller.ts          — bare call `foo()`
    //   * real-impl.ts       — production `function foo()`
    //   * test-fixture.test.ts — same-named local `function foo()`
    //
    // L4 (global-unique) used to count both `foo` symbols and either pick
    // them ambiguously or, if the test fixture was the lexically first
    // match, bind the caller there. After the fix L4's NOT-test-path
    // candidate filter leaves a single eligible target — the real impl —
    // so the bare call resolves correctly.
    let fx = index_fixture(&[
        ("src/caller.ts", "foo();\n"),
        ("src/real-impl.ts", "export function foo() { return 1; }\n"),
        (
            "src/__tests__/test-fixture.test.ts",
            "function foo() { return 999; }\nfoo();\n",
        ),
    ]);

    let refs = call_refs_to(&fx.repo, "foo");
    let from_caller: Vec<_> = refs
        .iter()
        .filter(|r| r.source_path == "src/caller.ts")
        .collect();

    assert_eq!(
        from_caller.len(),
        1,
        "expected exactly one ref from caller.ts to foo, got {from_caller:?}",
    );
    let r = from_caller[0];

    assert_eq!(
        r.target_path.as_deref(),
        Some("src/real-impl.ts"),
        "L4 must skip the test-fixture target and bind to the real impl, got {r:?}",
    );
    assert!(
        r.confidence >= 0.7,
        "expected confident resolution (>=0.7), got {r:?}",
    );
}

#[test]
fn l4_filter_covers_all_test_path_shapes() {
    // Each path shape that the filter knows about must, on its own,
    // disqualify a candidate so the lone non-test sibling wins L4.
    //
    // Both nested (`pkg/tests/foo.py`) and top-level (`tests/foo.py`)
    // dir shapes are exercised. FastAPI's `tests/main.py` (top-level)
    // was the bug that prompted the top-level patterns; pre-fix the
    // nested-only LIKE missed it and the test fixture won L4.
    let cases: &[(&str, &str)] = &[
        ("src/__tests__/dupe.ts", "__tests__ nested"),
        ("__tests__/dupe.ts", "__tests__ top-level"),
        ("src/dupe_test.ts", "_test"),
        ("src/dupe.test.ts", ".test"),
        ("src/dupe.spec.ts", ".spec"),
        ("src/tests/dupe.ts", "/tests/ nested"),
        ("tests/dupe.ts", "tests/ top-level"),
        ("tests/sub/dupe.ts", "tests/sub top-level"),
        ("src/test/dupe.ts", "/test/ nested"),
        ("test/dupe.ts", "test/ top-level"),
        ("playground/some/dupe.ts", "playground top-level"),
        ("src/playground/some/dupe.ts", "/playground/ nested"),
    ];

    for (test_path, label) in cases {
        let fx = index_fixture(&[
            ("src/c.ts", "uniqueShape();\n"),
            (
                "src/real-impl.ts",
                "export function uniqueShape() { return 1; }\n",
            ),
            (test_path, "function uniqueShape() { return 999; }\n"),
        ]);

        let refs = call_refs_to(&fx.repo, "uniqueShape");
        let from_caller: Vec<_> = refs
            .iter()
            .filter(|r| r.source_path == "src/c.ts")
            .collect();
        assert_eq!(
            from_caller.len(),
            1,
            "[{label}] expected one ref from caller, got {from_caller:?}",
        );
        let r = from_caller[0];
        assert_eq!(
            r.target_path.as_deref(),
            Some("src/real-impl.ts"),
            "[{label}] should bind to real-impl.ts despite test fixture at {test_path}",
        );
    }
}

#[test]
fn l4_filter_catches_fastapi_style_top_level_tests_dir() {
    // FastAPI dogfood (v0.5.0): `tests/main.py` defined a `State`
    // class that competed with `fastapi/applications.py::State`
    // for the L4 global-unique resolution. Pre-fix the LIKE
    // pattern only matched `%/tests/%` (nested), not `tests/%`
    // (top-level), so the test fixture was a valid candidate
    // and won the tie. Test asserts top-level `tests/foo.py`
    // and `tests/sub/bar.py` are both disqualified.
    let fx = index_fixture(&[
        ("src/caller.py", "uniqueState()\n"),
        (
            "src/fastapi/applications.py",
            "def uniqueState():\n    return 1\n",
        ),
        ("tests/main.py", "def uniqueState():\n    return 999\n"),
        ("tests/sub/bar.py", "def uniqueState():\n    return 1000\n"),
    ]);

    let refs = call_refs_to(&fx.repo, "uniqueState");
    let from_caller: Vec<_> = refs
        .iter()
        .filter(|r| r.source_path == "src/caller.py")
        .collect();
    assert_eq!(
        from_caller.len(),
        1,
        "expected one ref from src/caller.py, got {from_caller:?}",
    );
    let r = from_caller[0];
    assert_eq!(
        r.target_path.as_deref(),
        Some("src/fastapi/applications.py"),
        "must skip top-level tests/ candidates and bind to real impl, got {r:?}",
    );
    assert!(r.confidence >= 0.7, "expected gated resolution, got {r:?}");
}

#[test]
fn test_source_files_still_resolve_to_real_targets() {
    // The filter MUST be candidate-side only. A test file calling a real
    // function still resolves at L4 — we just don't pick test-path TARGETS.
    let fx = index_fixture(&[
        (
            "src/__tests__/caller.test.ts",
            "import { realFn } from '../impl';\nrealFn();\n",
        ),
        ("src/impl.ts", "export function realFn() { return 7; }\n"),
    ]);

    let refs = call_refs_to(&fx.repo, "realFn");
    let from_test: Vec<_> = refs
        .iter()
        .filter(|r| r.source_path == "src/__tests__/caller.test.ts")
        .collect();
    assert_eq!(
        from_test.len(),
        1,
        "test file's call to realFn should still resolve, got {from_test:?}",
    );
    let r = from_test[0];
    assert_eq!(r.target_path.as_deref(), Some("src/impl.ts"));
    assert!(r.confidence >= 0.7, "{r:?}");
}
