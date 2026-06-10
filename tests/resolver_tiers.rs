//! Resolution-tier contract tests against the SQL resolver (`batch_resolve_refs`).
//!
//! Tier ladder: L1 same-file 1.0 → L2 same-package 0.95 → L3 qualifier 0.9
//! → L4 global-unique 0.8 → L5 ambiguous 0.5 → unresolved 0.0.

mod common;
use common::*;

#[test]
fn tier1_same_file_resolves_at_1_0() {
    let fx = index_fixture(&[(
        "app/a.go",
        "package app\n\nfunc Helper() int { return 1 }\n\nfunc Caller() int { return Helper() }\n",
    )]);
    let refs = call_refs_to(&fx.repo, "Helper");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 1.0);
    assert_eq!(refs[0].target_path.as_deref(), Some("app/a.go"));
    assert_eq!(refs[0].source_symbol.as_deref(), Some("Caller"));
}

#[test]
fn tier2_same_package_resolves_at_0_95() {
    let fx = index_fixture(&[
        (
            "app/x.go",
            "package app\n\nfunc Shared() int { return 1 }\n",
        ),
        (
            "app/y.go",
            "package app\n\nfunc UseShared() int { return Shared() }\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "Shared");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("app/x.go"));
    assert_eq!(refs[0].source_symbol.as_deref(), Some("UseShared"));
}

#[test]
fn tier3_import_qualifier_disambiguates_at_0_9() {
    // QFn exists in two packages; the import of .../util must pick util/q.go.
    let fx = index_fixture(&[
        ("util/q.go", "package util\n\nfunc QFn() int { return 1 }\n"),
        (
            "other/q.go",
            "package other\n\nfunc QFn() int { return 2 }\n",
        ),
        (
            "main.go",
            "package main\n\nimport \"example.com/proj/util\"\n\nfunc M() int { return util.QFn() }\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "QFn");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.9);
    assert_eq!(refs[0].target_path.as_deref(), Some("util/q.go"));
    assert_eq!(refs[0].source_symbol.as_deref(), Some("M"));
}

#[test]
fn tier3_python_import_qualifier() {
    let fx = index_fixture(&[
        ("util.py", "def pfn():\n    return 1\n"),
        ("other/util2.py", "def pfn():\n    return 2\n"),
        // Caller lives in a different dir so the same-package tier can't win.
        (
            "app/main.py",
            "import util\n\ndef caller():\n    return util.pfn()\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "pfn");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.9);
    assert_eq!(refs[0].target_path.as_deref(), Some("util.py"));
}

#[test]
fn tier4_global_unique_resolves_at_0_8() {
    let fx = index_fixture(&[
        (
            "uniq/u.go",
            "package uniq\n\nfunc OnlyOnce() int { return 1 }\n",
        ),
        (
            "caller/c.go",
            "package caller\n\nfunc CallsU() int { return OnlyOnce() }\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "OnlyOnce");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.8);
    assert_eq!(refs[0].target_path.as_deref(), Some("uniq/u.go"));
}

#[test]
fn tier5_ambiguous_resolves_at_0_5() {
    let fx = index_fixture(&[
        ("a/d.go", "package a\n\nfunc Dup() int { return 1 }\n"),
        ("b/d.go", "package b\n\nfunc Dup() int { return 2 }\n"),
        (
            "caller/c.go",
            "package caller\n\nfunc C() int { return Dup() }\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "Dup");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.5);
    assert!(
        refs[0].target_path.is_some(),
        "ambiguous still picks a candidate"
    );
}

#[test]
fn unresolvable_stays_at_0_0() {
    let fx = index_fixture(&[(
        "main.go",
        "package main\n\nfunc M() int { return NoSuchFn() }\n",
    )]);
    let refs = call_refs_to(&fx.repo, "NoSuchFn");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.0);
    assert!(refs[0].target_path.is_none());
}

#[test]
fn imports_are_never_resolved_to_symbols() {
    let fx = index_fixture(&[(
        "main.go",
        "package main\n\nimport \"example.com/proj/util\"\n\nfunc M() int { return 0 }\n",
    )]);
    let conn = fx.repo.conn();
    let (count, resolved): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(target_file_id) FROM refs WHERE kind = 'import'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(resolved, 0);
}

#[test]
fn higher_tiers_win_over_lower_tiers() {
    // Same name in same file AND same package — same-file (1.0) must win.
    let fx = index_fixture(&[
        (
            "app/a.go",
            "package app\n\nfunc Pick() int { return 1 }\n\nfunc UsesPick() int { return Pick() }\n",
        ),
        ("app/b.go", "package app\n\nfunc Pick2() int { return 1 }\n"),
    ]);
    let refs = call_refs_to(&fx.repo, "Pick");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].confidence, 1.0);
    assert_eq!(refs[0].target_path.as_deref(), Some("app/a.go"));
}
