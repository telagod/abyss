//! Callers kind contract: `find_callers_filtered` (and the CLI / MCP it
//! backs) must surface type-position users by default. On TypeScript that
//! means an exported interface used as an annotation, type argument, or
//! `implements` clause counts as a caller. Pre-fix, `callers IFace` would
//! return zero rows even though three files type-depended on it.

mod common;
use code_abyss::graph::{CallerKindFilter, GraphQuery};
use common::*;

/// Three users of `IFace`, all type-position:
///   * annotates_iface.ts — `let x: IFace`
///   * implements_iface.ts — `class Impl implements IFace`
///   * generic_iface.ts — `function take<T extends IFace>(x: T)`
///
/// Plus one invocation user of a different symbol (`callDirect`) so the
/// fixture has both call and type_ref edges and the kind-filter assertions
/// have something to compare against.
fn iface_fixture() -> Fixture {
    index_fixture(&[
        (
            "src/types.ts",
            "export interface IFace { foo(): void }\n\
             export function callDirect(): number { return 1 }\n",
        ),
        (
            "src/annotates_iface.ts",
            "import { IFace } from './types';\nlet x: IFace | null = null;\n",
        ),
        (
            "src/implements_iface.ts",
            "import { IFace } from './types';\n\
             export class Impl implements IFace { foo(): void {} }\n",
        ),
        (
            "src/generic_iface.ts",
            "import { IFace } from './types';\n\
             export function take<T extends IFace>(x: T): T { return x }\n",
        ),
        (
            "src/calls_direct.ts",
            "import { callDirect } from './types';\n\
             export function go() { return callDirect() }\n",
        ),
    ])
}

#[test]
fn default_includes_type_ref_users() {
    // Pre-fix bug: default filter dropped kind='type_ref' rows, so an agent
    // running `callers IFace` on this fixture got zero callers despite 3
    // type-position users in the DB. The default filter MUST be the
    // superset — calls + field_access + type_ref.
    let fx = iface_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered("IFace", 20, 0.0, true)
        .expect("query");

    let kinds: Vec<&str> = result.callers.iter().map(|c| c.kind.as_str()).collect();
    assert!(
        result.callers.len() >= 3,
        "expected ≥3 callers for IFace (annotation, implements, generic), got {} ({kinds:?})",
        result.callers.len(),
    );
    assert!(
        kinds.iter().all(|k| *k == "type_ref"),
        "all IFace users should be type_ref, got {kinds:?}",
    );
}

#[test]
fn calls_only_filter_excludes_type_users() {
    // `--calls-only` must restore the legacy invocation-only behaviour:
    // pure type-position users disappear.
    let fx = iface_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered_kinds("IFace", 20, 0.0, true, CallerKindFilter::CallsOnly)
        .expect("query");

    assert_eq!(
        result.callers.len(),
        0,
        "calls-only must drop type_ref users for IFace, got {:?}",
        result.callers,
    );
}

#[test]
fn types_only_filter_includes_only_type_users() {
    // `--types-only` is the dual: invocation callers must disappear, type
    // users must remain. We assert on `callDirect`, which has one
    // invocation user in `calls_direct.ts` — types-only should drop it.
    let fx = iface_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered_kinds("callDirect", 20, 0.0, true, CallerKindFilter::TypesOnly)
        .expect("query");
    assert_eq!(
        result.callers.len(),
        0,
        "types-only must drop invocation users of callDirect, got {:?}",
        result.callers,
    );

    // And IFace users — all type_ref — survive.
    let result = gq
        .find_callers_filtered_kinds("IFace", 20, 0.0, true, CallerKindFilter::TypesOnly)
        .expect("query");
    assert!(
        result.callers.len() >= 3,
        "types-only must keep type_ref users of IFace, got {:?}",
        result.callers,
    );
}

#[test]
fn caller_rows_carry_kind_for_surface_formatting() {
    // The CLI suffixes mixed listings with `(call, 95%)` vs `(type, 95%)`.
    // That needs the kind on every row, not just an aggregate count.
    let fx = iface_fixture();
    let gq = GraphQuery::new(&fx.repo);

    let result = gq
        .find_callers_filtered("callDirect", 20, 0.0, true)
        .expect("query");
    assert!(
        !result.callers.is_empty(),
        "expected at least one invocation caller of callDirect",
    );
    for c in &result.callers {
        assert_eq!(
            c.kind, "call",
            "expected kind=call for callDirect invocation, got {:?}",
            c,
        );
    }
}
