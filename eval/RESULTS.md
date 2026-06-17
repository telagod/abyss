# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

> **2026-06-17 — same-language-family filter on the demoted tiers.**
> Cross-file resolution tiers L2/L3/L4/L4b/L5 now require the candidate's
> `files.lang_family` to equal the source file's. Found by dogfooding: a
> Rust `target()` call (petgraph edge endpoint) was claimed by an
> unrelated JS `function target()` because L5 ran a pure name match
> across the whole `symbols` table. Families: rust, go, python, ts
> (typescript+tsx+javascript), c (c+cpp), java, bash. Single-corpus eval
> impact should be ~zero — every corpus is single-language — but the
> demoted tiers on polyglot repos (Go/JS, Rust/JS, …) get noticeably
> cleaner. Binding-driven tiers (L0/L0b/L0c/L0d) and same-file tiers
> (L1/L4a) are unchanged.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-16, abyss v0.3.4-dev (five languages, six corpora)

| Corpus | Language | Truth pairs | Gated precision | Gated recall | All precision | All recall |
|--------|----------|------------:|----------------:|-------------:|--------------:|-----------:|
| gin v1.10.0 | Go | 2,968 | **99.3%** | **82.6%** | 89.2% | 88.0% |
| hono v4.6.14 | TypeScript | 5,611 | **98.8%** | **63.8%** | 77.7% | 76.3% |
| click 8.1.8 | Python | 573 | **98.7%** | **94.6%** | 97.5% | 96.2% |
| ripgrep 14.1.1 | Rust | 4,283 | **98.5%** | **75.3%** | 86.9% | 86.8% |
| abyss @8099aeb | Rust (dogfood) | 450 | **100.0%** | **90.9%** | 98.4% | 98.4% |
| cmark 0.31.1 | C | 1,383 | **99.1%** | **74.8%** | 99.1% | 74.8% |

Gated = `--min-confidence 0.7` (the default). abyss index time per corpus:
~150–900ms; the SCIP indexers take 40s–4min on the same machines.

### gin (Go) — scip-go

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 656 | 0 | **100%** |
| 0.95 | receiver-type match + same-package unique | 1,788 | 16 | 99.1% |
| 0.9 | import qualifier, unique candidate | 1 | 0 | 100% |
| 0.8 | global unique | 6 | 3 | 66.7% |
| 0.6 / 0.5 | demoted / ambiguous | 138 | 299 | 31.6% |

### hono (TypeScript) — scip-typescript

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 494 | 2 | **99.6%** |
| 0.95 | receiver-type (scope/binding/type-file) + import binding + same-package unique | 2,910 | 30 | 99.0% |
| 0.9 | import qualifier, unique candidate | 1 | 0 | 100% |
| 0.8 | global unique (member-shaped for qualified calls) | 175 | 10 | 94.6% |
| 0.6 / 0.5 | demoted / ambiguous | 704 | 1,186 | 37.2% |

### click (Python) — scip-python

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 189 | 1 | **99.5%** |
| 0.95 | receiver-type + named-import binding + same-package unique | 333 | 6 | 98.2% |
| 0.8 | global unique | 18 | 0 | 100% |
| 0.6 / 0.5 | demoted / ambiguous | 9 | 7 | 56.3% |

### ripgrep (Rust, third-party) — rust-analyzer scip

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 561 | 0 | **100%** |
| 0.95 | receiver-type (scope/binding/type-file) + use-binding | 2,397 | 19 | 99.2% |
| 0.8 | global unique | 269 | 29 | 90.3% |
| 0.6 / 0.5 | demoted / ambiguous | 492 | 515 | 48.9% |

### abyss (Rust, dogfood) — rust-analyzer scip

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 165 | 0 | **100%** |
| 0.95 | receiver-type (scope/binding/type-file) + use-binding | 154 | 0 | **100%** |
| 0.8 | global unique | 90 | 0 | **100%** |
| 0.6 / 0.5 | demoted / ambiguous | 34 | 7 | 82.9% |

### cmark (C) — scip-clang

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 373 | 9 | **97.6%** |
| 0.95 | receiver-type + include-binding + same-dir unique | 526 | 0 | **100%** |
| 0.8 | global unique | 135 | 0 | **100%** |

## How the eval drove the resolver (chronicle)

### Round 10 — 2026-06-16: C/C++ caller tracing — first C corpus

New `CExtractor` / `CppExtractor` sharing `c_cpp.rs`: direct calls, `.`/`->`
method calls, C++ `qualified_identifier` (`ns::func()`), `new` expressions,
`#include "..."` imports, `this` receiver inference, typed parameter/local
inference, `std::` namespace filtering, `base_class_clause` inheritance.

Chunker additions: `class_specifier`, `struct_specifier`, `namespace_definition`,
`type_definition`, `enum_specifier` as chunk boundaries + scope nodes. Fixed
`extract_node_name` for C/C++ `function_definition` (return type was winning
the fallback race over the function name inside `function_declarator`).

The `compare.py` SCIP name extractor needed a `cxx` scheme handler: scip-clang
symbols use `cxx . . $ descriptor` format (space-delimited prefix, no `/`
in simple descriptors), overload hashes in parens, and backtick-quoted macro
identifiers that must be skipped.

First run on cmark 0.31.1 (CommonMark reference parser, pure C, 38 source files,
1,383 truth pairs): **99.1% gated precision / 74.8% recall**. The 0.95 tier
(same-dir unique + receiver-type) is 100% — C's flat file structure makes
directory-based resolution very effective. The 340 unresolved calls are mostly
`static` helper functions in `.h` headers that SCIP defines in the header but
abyss resolves to the `.c` file that includes it — a "wrong file" by strict
join semantics but functionally correct. 85 tests, zero regression on existing
five corpora.

### Round 9 — 2026-06-12: the recall round — type-grade evidence beyond exact scope

TS and Rust recall were the laggards (58.7% / 67.8%). Dissecting everything
below the gate found the correct answers clustering in three places, each
with recoverable evidence:

1. **Non-exported function consts were invisible** (TS): bare
   `const splitRule = (...) => ...` at module level was only a symbol when
   wrapped in an `export_statement` (the only boundary). 187 unresolved
   bare calls in hono had same-file truth. `lexical_declaration` /
   `variable_declaration` are now chunk boundaries; hono unresolved
   dropped 286 → 99 and the 1.0 tier grew 301 → 494 correct.
2. **L0's exact-scope match was too rigid** (both): the receiver type was
   correctly inferred, but type aliases (`use X as Separator`), impls split
   across files, and trait-scoped methods never matched `symbols.scope`.
   Two new tiers turn the inferred type itself into file evidence:
   **L0c** — the source file imports the receiver type; the binding's
   target file carries the method name. **L0d** — the type symbol
   (class/struct/interface/enum) has a unique defining file and that file
   carries the method name.
3. **`impl Trait for Type` scoped methods to the trait** (Rust): the
   chunker's owner extraction grabbed the first type_identifier — the
   trait — so `impl Sink for StandardSink` methods got scope `Sink` and L0
   receiver matching never fired. Owner (and the oversized-impl scope
   stack) now use the impl's `type` field.

Net: hono **98.8% / 63.8%** (+5.1pp recall), ripgrep **98.5% / 75.3%**
(+7.5pp), dogfood **100.0% / 90.9%** (un-gated 98.4/98.4); gin and click
unchanged to the decimal. 74 tests.

### Round 8 — 2026-06-12: ripgrep — the third-party Rust verdict

The dogfood corpus was friendly; ripgrep 14.1.1 (13-crate workspace, 4,283
truth pairs) was the real exam. First run: 94.3%/68.4%. Three findings:

1. **Workspace crate roots** — `crate::` resolved against a hardcoded
   `src/`; workspace members live in `crates/<name>/src` (and ripgrep's
   core crate has NO src dir — `crates/core/main.rs` is a path-overridden
   root). `crate::` now walks the importing file's ancestors for a
   `Cargo.toml`, trying `<member>/src` then `<member>` itself.
2. **`super` inside an inline `mod tests`** is the file itself, not the
   parent dir — `use super::escape` in escape.rs's test module bound to
   lib.rs and made calls to `escape` *in its own file* wrong at 0.95. If
   the source file defines the item, a `super::` binding now resolves to
   the source file before any dir logic.
3. **Rust receiver lite inference** (the big one): typed parameters,
   `let x = T::new()` / `T { .. }` / annotated lets, and `self` → enclosing
   impl type. The 0.95 sub-tier audit after the fix: receiver-typed
   1,224/3, use-bindings 588/0 — but same-dir-unique qualified calls were
   **76%** (229/72). A Rust dir is not a namespace (files in one dir are
   separate modules needing `use`), unlike Go where the same slice measures
   98%. L2 now excludes qualified calls for Rust sources only; they fall to
   the 0.6 possibles.

Net: ripgrep 94.3/68.4 → **98.9% / 67.8%** (un-gated recall 85.3% — the
information is there, below the gate). The receiver inference also moved
the dogfood corpus from 86.7% to 84.2% recall (the L2 guard) at a flat
100.0% precision; gin/hono/click unchanged to the decimal.

### Round 7 — 2026-06-12: Rust corpus (dogfood) + the oversized-symbol bug

First Rust ground truth, via `rust-analyzer scip` on abyss itself. The
first run (96.7%/78.4%) exposed three gaps; fixing them took the corpus to
**100.0% gated precision / 86.7% recall, zero unresolved refs**:

1. **Oversized functions vanished from the symbols table** — the chunker's
   descend branch for >max_lines nodes emitted no symbol for the node
   itself, so a 120-line function was invisible to every resolver tier:
   bare same-file calls to `collect_refs` fell to the ambiguous tier and
   lost. The fix (a header chunk carrying the node's symbol) is
   language-agnostic — gin/hono/click each gained a few tenths from it.
2. **`T::new()` receiver type** — the path qualifier's last segment IS the
   receiver type. Every type has `new`; name tiers alone picked
   `Config::new` for eight different types. Now an associated-function call
   feeds L0 like any typed receiver.
3. **Rust `use` bindings** — `use crate::storage::Repository` (incl.
   `{A, B as C}` lists, `super::`/`self::` prefixes) now produce import
   bindings; module paths resolve against the files table
   (`x.rs` / `x/mod.rs`, crate root → `src/`). `pub use` re-exports look
   identical to imports, so the existing barrel chase follows
   `pub use repo::Repository` in `mod.rs` to the defining file for free.

Caveats stated plainly: the corpus is abyss itself (~76 files, 450 truth
pairs — small, and written by the people tuning the resolver); rust-analyzer
emits no occurrences for some macro-generated code. The numbers are real
but the corpus is friendly; a third-party Rust corpus is the obvious next
step before bragging.

### Round 6 — 2026-06-11: bindings for Python and Java

The TS binding tier (round 5) ported in one sitting:

- **Python**: `from .mod import a, b as c` → per-name bindings; relative
  modules resolve against the importing file's package (one leading dot =
  current package, each extra dot = one level up), absolute dotted paths
  match exactly or by *unique* path suffix (src-layouts). Candidates:
  `<base>.py`, `<base>/__init__.py`.
- **Java**: `import com.foo.Bar` binds the simple name `Bar` to the unique
  file ending `/com/foo/Bar.java` — disambiguates same-named classes across
  packages for constructor calls and type refs.

click: gated 98.1/90.8 → **98.7% / 94.2%** (both up; recall now above the
pre-round-4 92.8 with precision +0.9pp on top), all-metrics 97.5/95.8,
unresolved 18 → 10, tier 1.0 at 99.5%. gin/hono: zero change. Java has no
SCIP ground truth yet — bindings there are covered by contract tests only.

### Round 5 — 2026-06-11: named-import bindings (TypeScript)

The 0.8 global-unique tier was hono's worst gated tier (84.5%, 98 wrong).
Dissection found two distinct lies:

1. **47× `app.use()` → the JSX `use` hook.** hono-base assigns router verbs
   at runtime, so the only *static* `use` in the repo is the unrelated hook —
   "globally unique" picked it confidently. Measured: a qualified call
   resolving to an unscoped free function was 6% precision, vs 96.7% for
   member-shaped candidates. L4 now requires qualified calls to take only
   method/owner-scoped candidates.
2. **45× bare calls that were *named imports*.** `import { css } from
   '../helper/css'` then `css(...)` — the definition shape
   (`export const css = defaultContext.css`) is invisible to the chunker, so
   the name looked globally unique elsewhere. The extractor recorded the
   module path but threw the bindings away; `resolve_import` was dead code.

The fix is the new **L0b named-import binding tier**: the TS extractor
records `import { a, b as c } from './x'` (and `export { y } from './z'`
re-exports) as binding refs; the pipeline resolves module paths against the
files table (extension + index-file candidates, ESM `.js`→`.ts` rewrites, no
disk probing) and chases barrel chains to the defining file; bare calls
matching a binding resolve at 0.95, *before* the same-file tier. A partial
index (`refs(source_file_id, target_name) WHERE kind='import_binding'`)
keeps resolve time at ~600ms on hono's 41k refs.

Net (hono): gated 95.3/51.3 → **98.5% / 58.5%** — the binding tier absorbed
~750 correct resolutions at 98.9% precision, unresolved dropped 417 → 286,
and all-recall rose 66.0 → 73.1%. gin and click: zero change (no TS
bindings), zero regression. 62 tests.

### Round 4 — 2026-06-11: the 1.0 tier earns its name

The 1.0 tier was the *least* precise confident tier on hono (81.9%) — worse
than 0.95 and 0.9. Dissection by receiver shape across all three corpora
showed exactly where the lies lived:

| Call shape at 1.0 | Correct | Wrong | Precision |
|-------------------|--------:|------:|----------:|
| bare `foo()` | 1,086 | 4 | 99.6% |
| self-like `this/self/cls/super()` | 185 | 3 | 98.4% |
| qualified `x.foo()`, globally-unique name | 39 | 1 | 97.5% |
| qualified `x.foo()`, common name | 31 | 101 | **23.5%** |

Common-name qualified calls were being claimed by unrelated same-file
symbols: hono's `app.get()` matched a `new Proxy(...)` trap method named
`get`; gin's `x.Foo()` matched same-file free functions. Three fixes,
each verified on all corpora:

1. **L1 qualifier guard** — same-file 1.0 now only for bare calls and
   self-like receivers. Qualified leftovers fall to the qualifier/global
   tiers; a new L4a same-file fallback at 0.6 keeps them visible as
   possibles. Globally-unique names re-resolve via L4 at 0.8 — no loss.
2. **Python receiver lite inference** (mirror of Go/TS): annotated
   parameters (`ctx: Context`, incl. `"Context"` forward refs), `x = Type()`
   CapWord constructor assignments, `x: Type = ...`, and `self`/`cls` →
   enclosing class.
3. **Methods in small classes kept their owner** — a function nested in a
   class-like node is now a Method symbol with `scope` = the class. Before,
   Python/Rust methods only got a scope when the class was big enough to be
   split into per-method chunks (the chunk-scope backfill), so L0 was blind
   to every class that fit in one chunk.

The self-like exemption matters for inheritance: `self.fail()` inside
click's `Choice` has an inferred receiver type but no `Choice`-owned `fail`
(it lives on the base class `ParamType`, same file) — same-file 1.0 is the
right call, measured 98.4%.

Net: tier 1.0 is now 97.9–100% everywhere (was 81.9% on hono). Gated:
gin 98.2→**99.2%** P; hono 93.4→**95.3%** P, recall +0.3pp; click
97.8→**98.1%** P, recall 92.8→90.8% — the 2pp recall trade is calls whose
receiver genuinely cannot be typed statically (loop variables, factory
returns); they now sit at 0.6 as possibles instead of posing as facts.

### Round 3 — 2026-06-11: TypeScript corpus exposed two structural gaps

The first hono run scored a catastrophic **39.5% gated precision / 15.2%
recall**. Diagnosis and fixes, each verified by re-running all three corpora:

1. **Method-as-class-field symbols** — hono's `Context.text` etc. are arrow
   functions assigned to class fields (`text: TextRespond = (...) => ...`),
   not `method_definition` nodes. The chunker never made them symbols, so
   `c.text()` resolved "globally unique" to the wrong file 394×. Fixed:
   function-valued `public_field_definition` / `field_definition` /
   module-level `variable_declarator` (e.g. `export const useState = ...`)
   are now Method/Function symbols. Unresolved refs dropped 2,759 → 417.
2. **TS receiver inference** (mirror of the Go lite rules): typed parameters
   (`(c: Context)`), `const x = new T()`, and `this` → enclosing class.
   L0 resolves 921 hono calls.
3. **Typed receiver ⇒ type-consistent evidence only** — hono's `app.get()`
   is assigned at runtime (no static definition), and the same-file tier
   "resolved" it to the test file itself 185×. Name-only tiers (same-file /
   same-package / global-unique) now skip refs whose receiver type is known;
   if L0 finds no owned symbol, the ref demotes instead of gambling.
   This also pushed gin's 0.95 tier to 99.3% (was 98.3%).

Net: hono 39.5%/15.2% → **93.4%/51.0%**; gin precision +0.8pp (recall
−0.8pp from the stricter guard); click unchanged.

### Round 2 — 2026-06-11: receiver-type lite inference (Go)

L0 receiver-type tier runs before everything else: the extractor infers
static receiver types (`x.M()` where `x` is a method receiver, function
parameter, or local declared with a literal / `NewT()` constructor — no
data-flow, no interface resolution), and method definitions record their
owner type (`symbols.scope` = Go receiver / enclosing class). When exactly
one file defines a same-named symbol owned by that type, resolve at 0.95.

Effect on gin: gated recall **79.1% → 83.5%** while precision also rose —
the recall the round-1 demotion had sacrificed, bought back with type
evidence instead of gambling.

### Round 1 — 2026-06-10: demotion + GLOB fixes (Go)

First-ever run scored 85.6% gated precision. Three fixes: same-package
multi-candidate demotion (interface-method collisions `Bind`×86 resolved by
first-candidate guess), qualifier-tier uniqueness (build-tag variants), and
LIKE→GLOB (SQLite LIKE is ASCII case-insensitive: the *variable* `JSON`
matched the *file* `json.go`). Net: 85.6% → 97.2% gated precision.

## Known weaknesses (current)

1. **Dynamic/metaprogrammed methods** (TS): hono assigns router verbs
   (`app.get/post/use`) in a constructor loop — no static definition exists.
   These stay unresolved or demoted; they are the bulk of hono's recall gap
   (51%). Correctly unresolvable by static naming; an agent sees them in
   `possible_callers`.
2. **Interface dispatch** (Go): interface-typed receivers stay demoted at
   0.6 by design (`tier0_unknown_receiver_stays_demoted`). Resolving them
   needs interface-satisfaction analysis — compiler territory.
3. **JSX/general TS noise**: hono's 0.5 tier is still large (1,161 joined
   pairs); JSX runtime calls and non-imported common names dilute global
   tiers. Named-import bindings (round 5) cleared the re-export/barrel slice.
4. Java ground truth: TODO (scip-java needs a build; planned).

## Corpus

| Repo | Ref | Language | Indexer |
|------|-----|----------|---------|
| gin-gonic/gin | v1.10.0 | Go | scip-go |
| honojs/hono | v4.6.14 | TypeScript | scip-typescript |
| pallets/click | 8.1.8 | Python | scip-python |
| BurntSushi/ripgrep | 14.1.1 | Rust | rust-analyzer scip |
| telagod/abyss | 8099aeb | Rust | rust-analyzer scip |
| commonmark/cmark | 0.31.1 | C | scip-clang |
