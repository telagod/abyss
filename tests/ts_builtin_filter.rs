//! TS/JS built-in name filter for L4/L4b/L5 (B4). hono dogfood (2026-06-17):
//! `Set (interface)` in src/context.ts collected 17 "callers" from unrelated
//! files because L4/L5 saw `new Set()` and `map.has(...)` invocations and
//! globally name-matched them to the user-defined `Set`. JS prototypes are
//! invisible to the resolver — the only fix that scales is to refuse to
//! resolve a known built-in name globally for ts-family sources.
//!
//! Pinned contracts:
//!   1. A TS file using `new Set()` to mean the JS built-in MUST NOT
//!      generate an above-gate ref to a user's `Set` interface in another
//!      file.
//!   2. A TS file with its OWN local `Set` class still resolves same-file
//!      (L1 handles that — the filter only fires on cross-file global
//!      tiers).
//!   3. Non-TS sources are unaffected: a Rust file with a `Set` struct
//!      gets its normal L4 resolution.

mod common;
use common::*;

#[test]
fn ts_new_set_does_not_link_to_user_set_interface() {
    // foo.ts defines `class Set`. bar.ts uses `new Set()` to mean the JS
    // built-in. Pre-B4, L4 would link bar.ts → foo.ts at confidence 0.8.
    // Post-B4, the JS_TS_BUILTIN filter blocks `Set` for ts-family sources
    // at L4/L4b/L5 — the ref either stays unresolved (NULL target) or
    // resolves only below the 0.7 agent gate.
    let fx = index_fixture(&[
        ("foo.ts", "export class Set { add(x: number): void {} }\n"),
        (
            "bar.ts",
            "export function useNative(): void {\n  const s = new Set();\n}\n",
        ),
    ]);

    // Look at every ref to `Set` originating from bar.ts above the agent
    // gate (0.7). None of them must point at foo.ts.
    let refs = refs_to(&fx.repo, "Set");
    let bad: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "bar.ts"
                && r.confidence >= 0.7
                && r.target_path.as_deref() == Some("foo.ts")
        })
        .collect();
    assert!(
        bad.is_empty(),
        "bar.ts must not link to foo.ts:Set at >=0.7 confidence (JS built-in), got {bad:?}"
    );
}

#[test]
fn ts_local_set_class_resolves_via_l1() {
    // Defensive: if a TS file BOTH defines AND uses its own `Set` class
    // inside one file, L1 (same-file, confidence 1.0) still resolves —
    // the B4 filter only fires at the cross-file global tiers (L4/L4b/L5).
    let fx = index_fixture(&[(
        "local.ts",
        "export class Set { add(x: number): void {} }\n\
         export function makeOne(): Set { return new Set(); }\n",
    )]);

    let refs = refs_to(&fx.repo, "Set");
    let local: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "local.ts"
                && r.confidence >= 0.95
                && r.target_path.as_deref() == Some("local.ts")
        })
        .collect();
    assert!(
        !local.is_empty(),
        "same-file Set usage must still resolve at >=0.95, got refs={refs:?}"
    );
}

#[test]
fn rust_set_type_unaffected_by_builtin_guard() {
    // Symmetric guard: the filter must NOT apply to non-TS sources. A Rust
    // file annotating a parameter with its own `Set` type must resolve
    // normally — `Set` is not a Rust built-in.
    let fx = index_fixture(&[
        (
            "foo.rs",
            "pub struct Set;\nimpl Set { pub fn add(&self, x: u32) {} }\n",
        ),
        (
            "bar.rs",
            "use crate::foo::Set;\npub fn take(s: Set) { s.add(1); }\n",
        ),
    ]);

    // Expect at least one above-gate type_ref or call ref to `Set` from
    // bar.rs pointing at foo.rs.
    let refs = refs_to(&fx.repo, "Set");
    let rust_refs: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.source_path == "bar.rs"
                && r.confidence >= 0.7
                && r.target_path.as_deref() == Some("foo.rs")
        })
        .collect();
    assert!(
        !rust_refs.is_empty(),
        "Rust Set type ref must not be blocked by the ts-only builtin filter, got refs={refs:?}"
    );
}
