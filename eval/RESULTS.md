# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-11, abyss v0.3.2-dev (three languages)

| Corpus | Language | Truth pairs | Gated precision | Gated recall | All precision | All recall |
|--------|----------|------------:|----------------:|-------------:|--------------:|-----------:|
| gin v1.10.0 | Go | 2,968 | **98.2%** | **82.7%** | 89.1% | 87.2% |
| hono v4.6.14 | TypeScript | 5,611 | **93.4%** | **51.0%** | 70.8% | 65.6% |
| click 8.1.8 | Python | 573 | **97.8%** | **92.8%** | 96.0% | 93.0% |

Gated = `--min-confidence 0.7` (the default). abyss index time per corpus:
~150–900ms; the SCIP indexers take 40s–4min on the same machines.

### gin (Go) — scip-go

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (untyped receivers only) | 666 | 31 | 95.6% |
| 0.95 | receiver-type match + same-package unique | 1,785 | 12 | **99.3%** |
| 0.9 | import qualifier, unique candidate | 1 | 0 | 100% |
| 0.8 | global unique | 1 | 2 | 33.3% |
| 0.6 / 0.5 | demoted / ambiguous | 136 | 273 | 33.3% |

### hono (TypeScript) — scip-typescript

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (untyped receivers only) | 312 | 69 | 81.9% |
| 0.95 | receiver-type match + same-package unique | 2,020 | 35 | **98.3%** |
| 0.9 | import qualifier, unique candidate | 2 | 0 | 100% |
| 0.8 | global unique | 526 | 98 | 84.3% |
| 0.6 / 0.5 | demoted / ambiguous | 819 | 1,313 | 38.4% |

### click (Python) — scip-python

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file | 363 | 9 | 97.6% |
| 0.95 | same-package unique | 169 | 2 | **98.8%** |
| 0.6 / 0.5 | demoted / ambiguous | 1 | 11 | 8.3% |

## How the eval drove the resolver (chronicle)

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
3. **JSX/general TS noise**: hono's 0.5 tier is large (1,219 joined pairs);
   re-exports, barrel files, and JSX runtime calls dilute global tiers.
4. Java ground truth: TODO (scip-java needs a build; planned).

## Corpus

| Repo | Ref | Language | Indexer |
|------|-----|----------|---------|
| gin-gonic/gin | v1.10.0 | Go | scip-go |
| honojs/hono | v4.6.14 | TypeScript | scip-typescript |
| pallets/click | 8.1.8 | Python | scip-python |
