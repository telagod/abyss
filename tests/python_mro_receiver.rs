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
