//! Cross-language resolver pollution guard (v0.3.7, 2026-06-17).
//!
//! Before the same-language-family filter, a Rust `target()` call (petgraph
//! edge endpoint) was claimed by an unrelated JS `function target()` because
//! L5 (ambiguous global, conf 0.5) ran a pure name-match across every
//! `symbols` row regardless of language. The fix narrows L2/L3/L4/L4b/L5 to
//! candidates whose `lang_family` equals the source file's.
//!
//! Tests in this file pin the invariant:
//! - cross-language name collisions don't pollute results
//! - same-language resolution still works (we didn't over-filter)
//! - ts/tsx/js stay in one family so the routine TS-importing-JS case works

mod common;
use common::*;

/// The dogfood-found bug: a Rust call to `target()` (petgraph edge endpoint)
/// must NOT resolve to a JS `function target()`. Both names exist in the
/// repo; before the fix L5 claimed the JS file at confidence 0.5.
#[test]
fn rust_call_does_not_resolve_to_js_definition() {
    let fx = index_fixture(&[
        (
            "src/graph.rs",
            "pub struct Edge;\nimpl Edge {\n    pub fn target(&self) -> u32 { 0 }\n}\n\npub fn walk(e: &Edge) -> u32 {\n    e.target()\n}\n",
        ),
        // The decoy: a JS file with a same-named global function.
        (
            "npm/install.js",
            "function target() { return 'js' }\nmodule.exports = target\n",
        ),
        // Caller in a Rust file in a different dir — forces cross-file
        // resolution to fall into L4 (global-unique) or L5 (ambiguous) if
        // there were a same-language decoy. Here there's only the JS decoy.
        (
            "src/main.rs",
            "fn run() -> u32 {\n    let val = bare_target();\n    val\n}\n\nfn bare_target() -> u32 { 1 }\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "target")
        .into_iter()
        .filter(|r| r.source_path == "src/graph.rs")
        .collect();
    // The Rust call must not be resolved to npm/install.js.
    for r in &refs {
        assert!(
            r.target_path.as_deref() != Some("npm/install.js"),
            "Rust target() must not cross into JS: {refs:?}"
        );
    }
}

/// Symmetric: a JS call must not get claimed by a Rust definition either.
#[test]
fn js_call_does_not_resolve_to_rust_definition() {
    let fx = index_fixture(&[
        ("src/lib.rs", "pub fn shared() -> u32 { 1 }\n"),
        (
            "scripts/run.js",
            "function shared() { return 'js' }\nfunction main() { return shared() }\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "shared")
        .into_iter()
        .filter(|r| r.source_path == "scripts/run.js")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    // Either resolved to the local JS file or not resolved at all — but
    // never crossing into the Rust file.
    assert!(
        refs[0].target_path.as_deref() != Some("src/lib.rs"),
        "JS shared() must not cross into Rust: {refs:?}"
    );
}

/// Same-language resolution still works — we didn't over-filter. A Rust call
/// to a unique Rust definition in a different dir must still land at L4 (0.8,
/// global-unique).
#[test]
fn same_language_global_unique_still_resolves() {
    let fx = index_fixture(&[
        // Different dir so L2 (same-package) doesn't claim it — forces the
        // resolver into the global-unique tier we care about here.
        ("lib/util.rs", "pub fn only_in_rust() -> u32 { 1 }\n"),
        ("src/main.rs", "fn run() -> u32 { only_in_rust() }\n"),
        // A JS file with a totally different symbol — should be ignored.
        ("npm/install.js", "function noise() { return 'js' }\n"),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "only_in_rust")
        .into_iter()
        .filter(|r| r.source_path == "src/main.rs")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.8);
    assert_eq!(refs[0].target_path.as_deref(), Some("lib/util.rs"));
}

/// Same-package same-language resolution still works at 0.95 (L2). A polyglot
/// directory mixing Go and JS must not cause the Go resolution to fail.
#[test]
fn same_language_same_package_unique_still_resolves() {
    let fx = index_fixture(&[
        (
            "app/x.go",
            "package app\n\nfunc only_go_helper() int { return 1 }\n",
        ),
        (
            "app/y.go",
            "package app\n\nfunc UseHelper() int { return only_go_helper() }\n",
        ),
        // Decoy in the same dir, different language.
        (
            "app/script.js",
            "function only_go_helper() { return 'js' }\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "only_go_helper")
        .into_iter()
        .filter(|r| r.source_path == "app/y.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("app/x.go"));
}

/// ts-family POSITIVE test: a `.ts` file referencing a symbol defined in a
/// `.js` file (ESM-style import) must still resolve — TS / TSX / JS / JSX
/// all collapse into one family so the routine polyglot case works.
#[test]
fn ts_to_js_family_interop_resolves() {
    let fx = index_fixture(&[
        // The JS definition.
        (
            "lib/util.js",
            "export function family_shared() { return 1 }\n",
        ),
        // TS file calling a JS-defined function via a relative ESM import.
        // The L0b binding tier resolves this through the import_binding,
        // but a non-binding fallback should ALSO be allowed since they're
        // family-mates.
        (
            "src/main.ts",
            "import { family_shared } from '../lib/util.js'\nexport function run() { return family_shared() }\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "family_shared")
        .into_iter()
        .filter(|r| r.source_path == "src/main.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    // Must resolve confidently (binding tier) AND land on the JS file —
    // proving the cross-family filter treats ts/js as one family.
    assert!(
        refs[0].confidence >= 0.8,
        "ts→js binding must resolve confidently: {refs:?}"
    );
    assert_eq!(refs[0].target_path.as_deref(), Some("lib/util.js"));
}

/// ts-family POSITIVE test #2: even without an explicit import binding, a
/// global-unique ts→js resolution must succeed because they're family-mates.
#[test]
fn ts_to_js_family_global_unique_resolves() {
    let fx = index_fixture(&[
        ("lib/util.js", "export function only_in_js() { return 1 }\n"),
        (
            "src/runner.ts",
            "export function run() { return only_in_js() }\n",
        ),
        // Decoy in another language — must NOT be picked.
        ("src/lib.rs", "pub fn only_in_js() -> u32 { 99 }\n"),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "only_in_js")
        .into_iter()
        .filter(|r| r.source_path == "src/runner.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    // ts→js (same family) must resolve; the Rust file must NOT win the
    // global-unique tier (which now requires same family).
    assert_eq!(refs[0].target_path.as_deref(), Some("lib/util.js"));
    assert!(
        refs[0].confidence >= 0.8,
        "ts→js global unique should still land at L4 (0.8) or higher: {refs:?}"
    );
}

/// Ambiguous-global tier (L5) cross-language guard: when ONLY a different-
/// language candidate exists, the ref must stay unresolved instead of being
/// pointed at the wrong language file at confidence 0.5.
#[test]
fn ambiguous_global_filters_cross_language() {
    let fx = index_fixture(&[
        // The only definition is in a JS file.
        (
            "npm/lib.js",
            "function helper() { return 1 }\nfunction other_helper() { return 2 }\n",
        ),
        // Go caller — the L5 tier used to claim npm/lib.js at conf 0.5.
        (
            "app/main.go",
            "package app\n\nfunc M() int { return helper() }\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "helper")
        .into_iter()
        .filter(|r| r.source_path == "app/main.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    // Must NOT cross language: target_file_id should be None (unresolved).
    assert!(
        refs[0].target_path.is_none(),
        "cross-language-only candidate must leave the ref unresolved: {refs:?}"
    );
    assert_eq!(refs[0].confidence, 0.0);
}
