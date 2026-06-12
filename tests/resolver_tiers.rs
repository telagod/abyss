//! Resolution-tier contract tests against the SQL resolver (`batch_resolve_refs`).
//!
//! Tier ladder: L0 receiver-type 0.95 → L1 same-file 1.0 → L2 same-package-unique 0.95 → L3 qualifier 0.9
//! → L4 global-unique 0.8 → L4b same-package-multi 0.6 → L5 ambiguous 0.5
//! → unresolved 0.0.

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

#[test]
fn java_same_package_and_unique_resolution() {
    let fx = index_fixture(&[
        (
            "app/Helper.java",
            "package app;\n\npublic class Helper {\n    public static int compute() { return 1; }\n}\n",
        ),
        (
            "app/Service.java",
            "package app;\n\npublic class Service {\n    public int run() { return compute(); }\n}\n",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "compute");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("app/Helper.java"));
    assert_eq!(refs[0].source_symbol.as_deref(), Some("run"));
}

#[test]
fn same_package_multi_candidate_demotes_to_0_6() {
    // Render is defined in two files of the same package (interface-method
    // collision, the dominant error class in the gin eval) — must NOT be
    // reported at 0.95; demoted below the 0.7 gate instead.
    let fx = index_fixture(&[
        (
            "app/json.go",
            "package app

func Render() int { return 1 }
",
        ),
        (
            "app/xml.go",
            "package app

func Render() int { return 2 }
",
        ),
        (
            "app/caller.go",
            "package app

func Out() int { return Render() }
",
        ),
    ]);
    let refs = call_refs_to(&fx.repo, "Render");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.6);
    assert!(refs[0].target_path.is_some(), "still offered as a hint");
}

// ═══ L0b: named-import binding tier (v0.3.3) ═══

#[test]
fn python_from_import_binding_resolves_cross_package() {
    // `from util import pfn` + bare pfn(): the binding must beat the
    // ambiguous-global tier despite a same-named decoy elsewhere.
    let fx = index_fixture(&[
        ("util.py", "def pfn():\n    return 1\n"),
        ("other/decoy.py", "def pfn():\n    return 2\n"),
        (
            "app/main.py",
            "from util import pfn\n\ndef caller():\n    return pfn()\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "pfn")
        .into_iter()
        .filter(|r| r.source_path == "app/main.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("util.py"));
}

#[test]
fn python_relative_import_binding_resolves() {
    // `from .helpers import fmt` — one leading dot = the importing file's
    // package.
    let fx = index_fixture(&[
        ("pkg/helpers.py", "def fmt(s):\n    return s\n"),
        ("other/helpers.py", "def fmt(s):\n    return s + s\n"),
        (
            "pkg/main.py",
            "from .helpers import fmt\n\ndef caller():\n    return fmt('x')\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "fmt")
        .into_iter()
        .filter(|r| r.source_path == "pkg/main.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("pkg/helpers.py"));
}

#[test]
fn java_import_binding_resolves_constructor() {
    // `import com.foo.Helper` + `new Helper()` — the binding must pick
    // com/foo over a same-named class in another package.
    let fx = index_fixture(&[
        (
            "src/com/foo/Helper.java",
            "package com.foo;\n\npublic class Helper {\n    public int run() { return 1; }\n}\n",
        ),
        (
            "src/com/bar/Helper.java",
            "package com.bar;\n\npublic class Helper {\n    public int run() { return 2; }\n}\n",
        ),
        (
            "src/com/app/Main.java",
            "package com.app;\n\nimport com.foo.Helper;\n\npublic class Main {\n    public int go() { return new Helper().run(); }\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Helper")
        .into_iter()
        .filter(|r| r.source_path == "src/com/app/Main.java")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(
        refs[0].target_path.as_deref(),
        Some("src/com/foo/Helper.java")
    );
}

#[test]
fn ts_named_import_binding_beats_global_unique() {
    // `css` is imported from helper/index.ts whose definition shape
    // (`export const css = ctx.css`) is invisible to the chunker, while an
    // unrelated file defines a `css` symbol — global-unique used to claim
    // the wrong file. The import binding must win.
    let fx = index_fixture(&[
        ("jsx/css.ts", "export const css = (s: string) => s\n"),
        (
            "helper/index.ts",
            "const ctx = { css: (s: string) => s }\nexport const css = ctx.css\n",
        ),
        (
            "app/main.ts",
            "import { css } from '../helper/index'\nexport const go = () => css('x')\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "css")
        .into_iter()
        .filter(|r| r.source_path == "app/main.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("helper/index.ts"));
}

#[test]
fn ts_barrel_reexport_chain_resolves_to_definition() {
    // app imports from a barrel (`export { css } from './css'`) — the
    // binding must be chased through the barrel to the defining file.
    let fx = index_fixture(&[
        ("lib/css.ts", "export const css = (s: string) => s\n"),
        ("lib/index.ts", "export { css } from './css'\n"),
        (
            "app/main.ts",
            "import { css } from '../lib/index'\nexport const go = () => css('x')\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "css")
        .into_iter()
        .filter(|r| r.source_path == "app/main.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(
        refs[0].target_path.as_deref(),
        Some("lib/css.ts"),
        "barrel must be chased to the definition: {refs:?}"
    );
}

#[test]
fn qualified_call_never_takes_unscoped_global_unique() {
    // app.use('/') with an unknown receiver must not resolve to a free
    // function `use` in another file just because the name is globally
    // unique (hono: the JSX hook claimed app.use 47×, 6% precision).
    let fx = index_fixture(&[
        ("hooks/index.ts", "export const use = (x: string) => x\n"),
        ("app/main.ts", "export const t = (app) => app.use('/')\n"),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "use")
        .into_iter()
        .filter(|r| r.source_path == "app/main.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert!(
        refs[0].confidence < 0.7,
        "qualified call to unscoped free function must stay below the gate: {refs:?}"
    );
}

// ═══ L0: receiver-type tier (v0.3.2) ═══

#[test]
fn tier0_receiver_type_disambiguates_same_package_collision() {
    // Two types in one package both define Render() — the historical 0.6
    // demotion case. A typed receiver must pick the right file at 0.95.
    let fx = index_fixture(&[
        (
            "render/html.go",
            "package render\n\ntype HTML struct{}\n\nfunc (h *HTML) Render() error { return nil }\n",
        ),
        (
            "render/json.go",
            "package render\n\ntype JSON struct{}\n\nfunc (j *JSON) Render() error { return nil }\n",
        ),
        (
            "render/use.go",
            "package render\n\nfunc Use() error {\n\tr := &HTML{}\n\treturn r.Render()\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Render")
        .into_iter()
        .filter(|r| r.source_path == "render/use.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("render/html.go"));
}

#[test]
fn tier0_parameter_receiver_resolves_cross_package() {
    // Receiver type comes from a function parameter; method lives in another
    // package — name tiers alone would be ambiguous or demoted.
    let fx = index_fixture(&[
        (
            "engine/engine.go",
            "package engine\n\ntype Engine struct{}\n\nfunc (e *Engine) Start() {}\n",
        ),
        (
            "boot/other.go",
            "package boot\n\ntype Other struct{}\n\nfunc (o *Other) Start() {}\n",
        ),
        (
            "boot/boot.go",
            "package boot\n\nimport \"example.com/p/engine\"\n\nfunc Boot(e *engine.Engine) {\n\te.Start()\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Start")
        .into_iter()
        .filter(|r| r.source_path == "boot/boot.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("engine/engine.go"));
}

#[test]
fn tier0_method_receiver_var_resolves_sibling_method() {
    // Inside a method, calls through the receiver var resolve to the same
    // type's methods even when defined in another file with name collisions.
    let fx = index_fixture(&[
        (
            "ctx/context.go",
            "package ctx\n\ntype Context struct{}\n\nfunc (c *Context) Reset() {}\n",
        ),
        (
            "ctx/pool.go",
            "package ctx\n\ntype Pool struct{}\n\nfunc (p *Pool) Reset() {}\n",
        ),
        (
            "ctx/run.go",
            "package ctx\n\nfunc (c *Context) Run() {\n\tc.Reset()\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Reset")
        .into_iter()
        .filter(|r| r.source_path == "ctx/run.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("ctx/context.go"));
}

#[test]
fn tier0_constructor_inference_new_prefix() {
    // w := NewWidget() → w: Widget by constructor naming convention.
    let fx = index_fixture(&[
        (
            "ui/widget.go",
            "package ui\n\ntype Widget struct{}\n\nfunc NewWidget() *Widget { return &Widget{} }\n\nfunc (w *Widget) Draw() {}\n",
        ),
        (
            "ui/canvas.go",
            "package ui\n\ntype Canvas struct{}\n\nfunc (c *Canvas) Draw() {}\n",
        ),
        (
            "ui/app.go",
            "package ui\n\nfunc App() {\n\tw := NewWidget()\n\tw.Draw()\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Draw")
        .into_iter()
        .filter(|r| r.source_path == "ui/app.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("ui/widget.go"));
}

#[test]
fn tier0_unknown_receiver_stays_demoted() {
    // Interface-typed parameter: receiver type is NOT inferrable (lite has no
    // interface resolution) — the collision must stay at the 0.6 demotion,
    // never a confident wrong answer.
    let fx = index_fixture(&[
        (
            "shape/circle.go",
            "package shape\n\ntype Circle struct{}\n\nfunc (c *Circle) Area() int { return 1 }\n",
        ),
        (
            "shape/square.go",
            "package shape\n\ntype Square struct{}\n\nfunc (s *Square) Area() int { return 2 }\n",
        ),
        (
            "shape/calc.go",
            "package shape\n\ntype Shaper interface{ Area() int }\n\nfunc Calc(s Shaper) int {\n\treturn s.Area()\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "Area")
        .into_iter()
        .filter(|r| r.source_path == "shape/calc.go")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert!(
        refs[0].confidence <= 0.6,
        "interface dispatch must not be confidently resolved: {refs:?}"
    );
}

// ═══ L0 receiver tier: TypeScript (v0.3.2) ═══

#[test]
fn tier0_ts_class_field_method_and_new_inference() {
    // Method-as-class-field (`text = (...) => ...`) must be a symbol, and
    // `const c = new Context()` must type the receiver.
    let fx = index_fixture(&[
        (
            "src/context.ts",
            "export class Context {\n  text = (s: string) => { return s }\n}\n",
        ),
        (
            "src/request.ts",
            "export class HonoRequest {\n  text = (s: string) => { return s }\n}\n",
        ),
        (
            "src/app.ts",
            "import { Context } from './context'\nexport const run = () => {\n  const c = new Context()\n  return c.text('hi')\n}\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "text")
        .into_iter()
        .filter(|r| r.source_path == "src/app.ts")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("src/context.ts"));
}

// ═══ L1 qualifier guard + L4a same-file fallback (v0.3.3) ═══

#[test]
fn qualified_unknown_receiver_same_file_demotes_to_0_6() {
    // x.Foo() where the receiver type is unknown and a free function Foo
    // happens to live in the same file: measured 23.5% precision across
    // corpora — must NOT be claimed at 1.0; falls to the 0.6 same-file
    // fallback below the gate.
    let fx = index_fixture(&[(
        "app/a.go",
        "package app\n\nfunc Handle() int { return 1 }\n\nfunc Use(x SomeIface) int { return x.Handle() }\n",
    )]);
    let refs = call_refs_to(&fx.repo, "Handle");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(
        refs[0].confidence, 0.6,
        "qualified call must not get same-file 1.0"
    );
    assert_eq!(refs[0].target_path.as_deref(), Some("app/a.go"));
}

#[test]
fn python_self_inherited_method_resolves_same_file_at_1_0() {
    // self.fail() inside Choice where fail is defined on the BASE class
    // ParamType in the same file (the click pattern): L0 finds no
    // Choice-owned `fail`, but the self-like exemption keeps same-file 1.0.
    let fx = index_fixture(&[(
        "types.py",
        "class ParamType:\n    def fail(self, msg):\n        raise ValueError(msg)\n\nclass Choice(ParamType):\n    def convert(self, value):\n        self.fail('bad')\n",
    )]);
    let refs = call_refs_to(&fx.repo, "fail");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(
        refs[0].confidence, 1.0,
        "self-like inherited call keeps 1.0: {refs:?}"
    );
    assert_eq!(refs[0].target_path.as_deref(), Some("types.py"));
}

// ═══ L0 receiver tier: Python (v0.3.3) ═══

#[test]
fn tier0_python_annotated_parameter_resolves_cross_file() {
    // def run(ctx: Context) → ctx.invoke() resolves to Context's file even
    // though `invoke` exists on two classes in the package.
    let fx = index_fixture(&[
        (
            "pkg/core.py",
            "class Context:\n    def invoke(self):\n        return 1\n",
        ),
        (
            "pkg/other.py",
            "class Runner:\n    def invoke(self):\n        return 2\n",
        ),
        (
            "pkg/use.py",
            "def run(ctx: Context):\n    return ctx.invoke()\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "invoke")
        .into_iter()
        .filter(|r| r.source_path == "pkg/use.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("pkg/core.py"));
}

#[test]
fn tier0_python_constructor_assignment_inference() {
    // ed = Editor() → ed: Editor by the CapWord constructor convention.
    let fx = index_fixture(&[
        (
            "pkg/editor.py",
            "class Editor:\n    def edit_file(self, name):\n        return name\n",
        ),
        (
            "pkg/pager.py",
            "class Pager:\n    def edit_file(self, name):\n        return name\n",
        ),
        (
            "pkg/use.py",
            "def open_editor():\n    ed = Editor()\n    return ed.edit_file('x')\n",
        ),
    ]);
    let refs: Vec<_> = call_refs_to(&fx.repo, "edit_file")
        .into_iter()
        .filter(|r| r.source_path == "pkg/use.py")
        .collect();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].confidence, 0.95);
    assert_eq!(refs[0].target_path.as_deref(), Some("pkg/editor.py"));
}

#[test]
fn tier0_ts_typed_receiver_without_definition_demotes() {
    // Receiver type known but the method has no static definition anywhere
    // (hono's runtime-assigned router verbs): name-only tiers must NOT claim
    // it — especially not same-file.
    let fx = index_fixture(&[
        (
            "src/hono.ts",
            "export class Hono {\n  route = (p: string) => { return p }\n}\n",
        ),
        (
            "src/app.test.ts",
            "import { Hono } from './hono'\nexport const get = (x: string) => x\nexport const t = () => {\n  const app = new Hono()\n  app.get('/')\n  return get('y')\n}\n",
        ),
    ]);
    let mut confidences: Vec<f64> = call_refs_to(&fx.repo, "get")
        .into_iter()
        .filter(|r| r.source_path == "src/app.test.ts")
        .map(|r| r.confidence)
        .collect();
    confidences.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // Two refs: the direct `get('y')` call resolves same-file at 1.0 (the
    // local const IS a symbol now); `app.get('/')` — typed receiver Hono
    // with no Hono-owned `get` — must stay below the gate, never 1.0.
    assert_eq!(confidences.len(), 2, "{confidences:?}");
    assert!(
        confidences[0] < 0.7,
        "typed receiver without owned symbol must demote: {confidences:?}"
    );
    assert_eq!(confidences[1], 1.0, "{confidences:?}");
}
