//! Python MRO-aware receiver inference (L0e resolver tier).
//!
//! `group: Group = Group(...); group.invoke(ctx)` — when `invoke` is defined
//! on `Command` and `Group` inherits it, the L0e tier walks the inheritance
//! chain (DFS via the `inherit` refs emitted by the Python extractor) and
//! resolves the call to the base class's file at confidence 0.95.

mod common;
use common::*;

#[test]
fn single_inheritance_resolves_to_base_file() {
    // Base defines foo; Derived inherits; caller invokes on Derived
    // instance. Without MRO, L0/L0c/L0d all miss (no `foo` scoped to
    // Derived) and the call falls through to lower tiers — typically a
    // wrong same-file match or an ambiguous global. With MRO, it lands on
    // base.py at 0.95.
    let fx = index_fixture(&[
        (
            "base.py",
            "class Base:\n    def foo(self):\n        return 1\n",
        ),
        (
            "derived.py",
            "from base import Base\n\nclass Derived(Base):\n    pass\n",
        ),
        (
            "caller.py",
            "from derived import Derived\n\ndef run():\n    d: Derived = Derived()\n    return d.foo()\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "foo");
    // Filter to just the caller.py ref (Base.foo's same-file definition
    // doesn't generate a call ref).
    let caller_refs: Vec<_> = refs
        .iter()
        .filter(|r| r.source_path == "caller.py")
        .collect();
    assert_eq!(caller_refs.len(), 1, "{refs:?}");
    assert_eq!(caller_refs[0].confidence, 0.95);
    assert_eq!(caller_refs[0].target_path.as_deref(), Some("base.py"));
    assert_eq!(caller_refs[0].source_symbol.as_deref(), Some("run"));
}

#[test]
fn three_level_chain_walks_to_root() {
    // Base ← Mid ← Leaf, only Base defines bar(). Calling bar() on a Leaf
    // instance must traverse two `inherit` hops to land on base.py.
    let fx = index_fixture(&[
        (
            "base.py",
            "class Base:\n    def bar(self):\n        return 1\n",
        ),
        (
            "mid.py",
            "from base import Base\n\nclass Mid(Base):\n    pass\n",
        ),
        (
            "leaf.py",
            "from mid import Mid\n\nclass Leaf(Mid):\n    pass\n",
        ),
        (
            "caller.py",
            "from leaf import Leaf\n\ndef run():\n    x: Leaf = Leaf()\n    return x.bar()\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "bar")
        .into_iter()
        .filter(|r| r.source_path == "caller.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("base.py"));
}

#[test]
fn unknown_method_stays_unresolved() {
    // bar() is defined nowhere. The MRO walker must not invent a target;
    // the ref either stays at 0.0 or falls through to a below-gate tier,
    // but must NEVER land on the type's own file at >=0.7. (No false
    // positive.)
    let fx = index_fixture(&[(
        "only.py",
        "class X:\n    pass\n\ndef run():\n    x: X = X()\n    return x.bar()\n",
    )]);
    let refs = call_refs_to(&fx.repo, "bar");
    // Either zero refs (everything dropped) or a single ref at confidence
    // < 0.7 (below the agent-facing gate). Anything >= 0.7 is a false
    // positive.
    for r in &refs {
        assert!(
            r.confidence < 0.7,
            "unknown method must not resolve at >=0.7: {r:?}"
        );
    }
}

#[test]
fn sibling_class_name_collision_picks_nearest_file() {
    // Django dogfood (2026-06-17): `DatabaseSchemaEditor` is defined in 4
    // sibling backend dirs (oracle/mysql/sqlite3/postgresql). Pre-B2, L0e
    // walked `class_files` in SQL row order — postgresql usually won — and
    // a `self.execute()` call inside `oracle/feature.py` resolved to
    // `postgresql/schema.py`. Wrong file, agent off chasing the wrong
    // implementation.
    //
    // The fix sorts L0e candidates by path-segment distance to the source:
    // a caller in `oracle/` finds the `oracle/` definition first; same
    // for mysql / postgresql callers. Symmetric and dogfood-correct.
    //
    // Each backend defines its own `DatabaseSchemaEditor(Base)` where Base
    // (`BaseSchemaEditor`) is the shared parent that owns `execute()`. So
    // a self.execute() call MUST land on base.py — and crucially the walk
    // must START from the SAME backend's DatabaseSchemaEditor, not a
    // sibling's.
    //
    // We assert via the caller_path / source_symbol pair: the walker
    // produces ONE resolved ref per call site, all landing on base.py.
    // The dogfood-broken behaviour was visible because the walker would
    // resolve via the wrong sibling — but with shared base.py here both
    // wrong and right pick base.py. So we add a per-backend method that
    // is ONLY defined on the local backend's Base, so the walk must
    // anchor on the right backend.
    let fx = index_fixture(&[
        (
            "base.py",
            "class BaseSchemaEditor:\n    def execute(self):\n        return 'base'\n",
        ),
        // Oracle backend: its own BaseSchemaEditor mixin defines a method
        // not on the shared BaseSchemaEditor. Anchoring on oracle's
        // DatabaseSchemaEditor walks oracle's chain; mis-anchoring on
        // postgresql's would miss it.
        (
            "oracle/base.py",
            "from base import BaseSchemaEditor\n\nclass OracleMixin(BaseSchemaEditor):\n    def oracle_only(self):\n        return 'oracle'\n",
        ),
        (
            "oracle/schema.py",
            "from oracle.base import OracleMixin\n\nclass DatabaseSchemaEditor(OracleMixin):\n    pass\n",
        ),
        (
            "oracle/feature.py",
            "from oracle.schema import DatabaseSchemaEditor\n\ndef run():\n    s: DatabaseSchemaEditor = DatabaseSchemaEditor()\n    return s.oracle_only()\n",
        ),
        // postgresql sibling — same class name DatabaseSchemaEditor with
        // its OWN parallel parent that does NOT have `oracle_only`. If the
        // walker picks the postgresql start_fid for an oracle caller, the
        // walk misses oracle_only and we either get an unresolved ref or
        // a different file. The test asserts the right file.
        (
            "postgresql/base.py",
            "from base import BaseSchemaEditor\n\nclass PostgresMixin(BaseSchemaEditor):\n    def pg_only(self):\n        return 'pg'\n",
        ),
        (
            "postgresql/schema.py",
            "from postgresql.base import PostgresMixin\n\nclass DatabaseSchemaEditor(PostgresMixin):\n    pass\n",
        ),
        (
            "mysql/base.py",
            "from base import BaseSchemaEditor\n\nclass MysqlMixin(BaseSchemaEditor):\n    def mysql_only(self):\n        return 'mysql'\n",
        ),
        (
            "mysql/schema.py",
            "from mysql.base import MysqlMixin\n\nclass DatabaseSchemaEditor(MysqlMixin):\n    pass\n",
        ),
    ]);

    // oracle_only is defined only on oracle/base.py:OracleMixin. If the
    // walker picked postgresql's DatabaseSchemaEditor for the oracle
    // caller, the walk would never see OracleMixin and the ref would
    // either go unresolved or land somewhere wrong. With B2 the nearest-
    // file ordering forces the oracle-anchored walk; ref resolves to
    // oracle/base.py at 0.95.
    let refs: Vec<_> = call_refs_to(&fx.repo, "oracle_only")
        .into_iter()
        .filter(|r| r.source_path == "oracle/feature.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("oracle/base.py"));
}

#[test]
fn multi_base_picks_left_first() {
    // C(A, B) where both A and B exist. Each base contributes a distinct
    // method — m on A, m_b on B. C().m() lands on a.py, C().m_b() lands on
    // b.py. This is the dominant multi-inheritance shape (mixins where one
    // base owns a given method).
    let fx = index_fixture(&[
        ("a.py", "class A:\n    def m(self):\n        return 1\n"),
        ("b.py", "class B:\n    def m_b(self):\n        return 2\n"),
        (
            "c.py",
            "from a import A\nfrom b import B\n\nclass C(A, B):\n    pass\n",
        ),
        (
            "caller.py",
            "from c import C\n\ndef run():\n    c: C = C()\n    c.m()\n    c.m_b()\n",
        ),
    ]);

    let m_refs: Vec<_> = call_refs_to(&fx.repo, "m")
        .into_iter()
        .filter(|r| r.source_path == "caller.py")
        .collect();
    assert_eq!(m_refs.len(), 1, "{m_refs:?}");
    assert_eq!(m_refs[0].confidence, 0.95);
    assert_eq!(m_refs[0].target_path.as_deref(), Some("a.py"));

    let mb_refs: Vec<_> = call_refs_to(&fx.repo, "m_b")
        .into_iter()
        .filter(|r| r.source_path == "caller.py")
        .collect();
    assert_eq!(mb_refs.len(), 1, "{mb_refs:?}");
    assert_eq!(mb_refs[0].confidence, 0.95);
    assert_eq!(mb_refs[0].target_path.as_deref(), Some("b.py"));
}
