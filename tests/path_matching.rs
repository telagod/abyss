//! `find_file_fuzzy` rules: the resolver an agent leans on every time it
//! types `abyss where <file>` / `abyss context <file>`. The bug it fixes:
//! a query like `src/hono.ts` used to bind to a vendored `benchmarks/jsx/
//! src/hono.ts` because the SQL was `path = ? OR path LIKE '%' || ?` with
//! no ranking — first-row-wins is insertion-order roulette.
//!
//! Priority (locked here):
//!   1. exact `path = query`            → unique winner.
//!   2. paths starting with `query`     → shortest first.
//!   3. paths ending with `query`       → shortest first
//!      (root-level beats deep nested copies).

mod common;

use common::*;

/// Three files named `x.ts` at different depths. The bug repro: ask for
/// the bare suffix and confirm the root copy wins, not the deepest one.
fn xts_fixture() -> Fixture {
    index_fixture(&[
        ("a/b/x.ts", "export const a = 1;\n"),
        ("x.ts", "export const root = 1;\n"),
        ("deep/nested/dir/x.ts", "export const deep = 1;\n"),
    ])
}

#[test]
fn suffix_query_returns_shortest_path() {
    // `abyss where x.ts` on the dogfood case: agents expect the canonical
    // root-level file, not whichever copy SQLite happened to scan first.
    let fx = xts_fixture();
    let hit = fx.repo.find_file_fuzzy("x.ts").unwrap();
    assert!(hit.is_some(), "x.ts must resolve");
    let (_id, path) = hit.unwrap();
    assert_eq!(path, "x.ts", "shortest-path winner (got {path})");
}

#[test]
fn exact_path_returns_exact_match() {
    // Exact matches must short-circuit — agents pasting a full relative path
    // should never get a near-miss masquerading as a hit.
    let fx = xts_fixture();
    let (_id, path) = fx.repo.find_file_fuzzy("a/b/x.ts").unwrap().unwrap();
    assert_eq!(path, "a/b/x.ts");
}

#[test]
fn deeper_suffix_query_resolves_to_deeper_file() {
    // `nested/dir/x.ts` is a unique suffix only of the deep copy — so even
    // though the root `x.ts` is shorter, the deep one wins because the root
    // doesn't match the longer suffix at all.
    let fx = xts_fixture();
    let (_id, path) = fx.repo.find_file_fuzzy("nested/dir/x.ts").unwrap().unwrap();
    assert_eq!(path, "deep/nested/dir/x.ts");
}

#[test]
fn root_anchored_query_outranks_vendored_copy() {
    // The hono-style trap: a real `src/hono.ts` plus a benchmark vendored
    // `benchmarks/jsx/src/hono.ts`. Agents typing `src/hono.ts` always mean
    // the framework's, never the benchmark's.
    let fx = index_fixture(&[
        ("src/hono.ts", "export class Hono {}\n"),
        ("benchmarks/jsx/src/hono.ts", "export class Vendored {}\n"),
    ]);
    let (_id, path) = fx.repo.find_file_fuzzy("src/hono.ts").unwrap().unwrap();
    assert_eq!(path, "src/hono.ts");
}

#[test]
fn missing_file_returns_none() {
    let fx = xts_fixture();
    assert!(
        fx.repo
            .find_file_fuzzy("does/not/exist.ts")
            .unwrap()
            .is_none(),
        "missing path must return None — never panic, never false-positive"
    );
}
