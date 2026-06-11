# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-11, abyss v0.3.3-dev (three languages)

| Corpus | Language | Truth pairs | Gated precision | Gated recall | All precision | All recall |
|--------|----------|------------:|----------------:|-------------:|--------------:|-----------:|
| gin v1.10.0 | Go | 2,968 | **99.2%** | **82.6%** | 89.1% | 87.2% |
| hono v4.6.14 | TypeScript | 5,611 | **98.5%** | **58.5%** | 77.1% | 73.1% |
| click 8.1.8 | Python | 573 | **98.1%** | **90.8%** | 95.5% | 92.5% |

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
| 1.0 | same file (bare + self-like calls) | 301 | 2 | **99.3%** |
| 0.95 | receiver-type + named-import binding + same-package unique | 2,795 | 30 | 98.9% |
| 0.9 | import qualifier, unique candidate | 2 | 0 | 100% |
| 0.8 | global unique (member-shaped for qualified calls) | 185 | 18 | 91.1% |
| 0.6 / 0.5 | demoted / ambiguous | 821 | 1,171 | 41.2% |

### click (Python) — scip-python

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 189 | 4 | **97.9%** |
| 0.95 | receiver-type match + same-package unique | 313 | 6 | 98.1% |
| 0.8 | global unique | 18 | 0 | 100% |
| 0.6 / 0.5 | demoted / ambiguous | 10 | 15 | 40.0% |

## How the eval drove the resolver (chronicle)

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
