//! Rust collection-name filter for L2/L4/L4b/L5 (v0.5.4). Mirrors the TS
//! built-in guard (v0.5.1 B4): a Rust file calling `Vec::new()` or
//! `Box::new()` has no symbol in the user's index for the std-lib types.
//! When a user-defined `struct Vec` or `enum Result` happens to exist
//! somewhere in the workspace (test fixtures, mocks, vendored deps that
//! escaped the walker), L2/L4/L5 will name-match the std-lib reference
//! to the user type — polluting the call graph.
//!
//! Unlike the TS case (`new Set()` extracts `Set` as the call target),
//! Rust paths like `Vec::new()` extract `new` as the target and `Vec`
//! as the qualifier. The guard blocks BOTH shapes so:
//!   * `Vec` as a TypeRef target_name is filtered, AND
//!   * `Vec::new()` (target_qualifier=Vec) is filtered.
//!
//! Pinned contracts:
//!   1. A Rust file using `Vec::new()` to mean the std-lib type MUST NOT
//!      generate an above-gate ref pointing at a user's `Vec` struct's
//!      `new` in another file.
//!   2. A Rust file with its OWN local `Vec` struct in the SAME file
//!      still resolves same-file (L1 doesn't run the guard).
//!   3. Non-Rust sources are unaffected: a Python file with a `Vec`
//!      class gets normal L4 resolution.

mod common;
use common::*;

#[test]
fn rust_vec_call_does_not_link_to_user_vec_struct() {
    // foo.rs defines `struct Vec` with `new()`. bar.rs uses `Vec::new()`
    // to mean the std-lib type. Pre-v0.5.4, L2 (same-dir-unique-`new`)
    // would link bar.rs → foo.rs at 0.95. Post-v0.5.4, the
    // RUST_BUILTIN_GUARD blocks any ref whose qualifier is `Vec` for
    // rust-family sources at L2/L4/L4b/L5 — the ref stays unresolved
    // or resolves only below the 0.7 agent gate.
    let fx = index_fixture(&[
        (
            "foo.rs",
            "pub struct Vec;\nimpl Vec { pub fn new() -> Self { Vec } }\n",
        ),
        (
            "bar.rs",
            "pub fn use_native() {\n    let _v: std::vec::Vec<u32> = Vec::new();\n}\n",
        ),
    ]);

    // The path call `Vec::new()` extracts as target_name=new, qualifier=Vec.
    // Above-gate (0.7) it must NOT resolve to foo.rs's `new`.
    let refs = refs_to(&fx.repo, "new");
    let bad: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "bar.rs"
                && r.confidence >= 0.7
                && r.target_path.as_deref() == Some("foo.rs")
        })
        .collect();
    assert!(
        bad.is_empty(),
        "bar.rs Vec::new() must not resolve to foo.rs's new at >=0.7 (Rust std-lib), got {bad:?}"
    );
}

#[test]
fn rust_box_new_does_not_link_to_user_box_struct() {
    // Same shape, different builtin. foo.rs defines its own `Box`;
    // bar.rs uses `Box::new(...)` for std-lib heap alloc.
    let fx = index_fixture(&[
        (
            "foo.rs",
            "pub struct Box;\nimpl Box { pub fn wrap() -> Self { Box } }\n",
        ),
        ("bar.rs", "pub fn make() {\n    let _b = Box::wrap();\n}\n"),
    ]);

    let refs = refs_to(&fx.repo, "wrap");
    let bad: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "bar.rs"
                && r.confidence >= 0.7
                && r.target_path.as_deref() == Some("foo.rs")
        })
        .collect();
    assert!(
        bad.is_empty(),
        "bar.rs Box::wrap() must not resolve via global-unique under the Rust guard, got {bad:?}",
    );
}

#[test]
fn rust_local_vec_resolves_same_file_via_l1() {
    // Defensive: a Rust file that BOTH defines AND uses its own `Vec`
    // struct inside ONE file — L1 (same-file, confidence 1.0) still
    // resolves. The v0.5.4 filter only fires at the cross-file global
    // tiers (L2/L4/L4b/L5), and L1 doesn't run the guard.
    let fx = index_fixture(&[(
        "local.rs",
        "pub struct LocalThing;\nimpl LocalThing { pub fn make() -> Self { LocalThing } }\n\
         pub fn driver() {\n    let _x = LocalThing::make();\n}\n",
    )]);

    // LocalThing isn't a builtin — confirms L1 same-file path works
    // for non-builtin user types. (Pure regression guard so we don't
    // accidentally over-filter L1.)
    let refs = refs_to(&fx.repo, "make");
    let local: Vec<_> = refs
        .iter()
        .filter(|r| r.source_path == "local.rs" && r.target_path.as_deref() == Some("local.rs"))
        .collect();
    assert!(
        !local.is_empty(),
        "same-file LocalThing::make() must still resolve, got refs={refs:?}"
    );
}

#[test]
fn python_vec_class_unaffected_by_rust_guard() {
    // Symmetric guard: the filter must NOT apply to non-Rust sources.
    // A Python file using its own `Vec` class must resolve normally —
    // `Vec` is not a Python built-in.
    let fx = index_fixture(&[
        ("foo.py", "class Vec:\n    pass\n"),
        (
            "bar.py",
            "from foo import Vec\n\ndef take():\n    v = Vec()\n    return v\n",
        ),
    ]);

    let refs = refs_to(&fx.repo, "Vec");
    let py_refs: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "bar.py"
                && r.confidence >= 0.7
                && r.target_path.as_deref() == Some("foo.py")
        })
        .collect();
    assert!(
        !py_refs.is_empty(),
        "Python Vec ref must not be blocked by the rust-only builtin guard, got refs={refs:?}"
    );
}
