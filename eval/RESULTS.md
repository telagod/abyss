# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-11, abyss v0.3.2-dev (receiver-type lite inference)

### gin v1.10.0 (Go, 102 files) — ground truth: scip-go

2,968 ground-truth call pairs · abyss index time: **~150ms** (scip-go: ~40s)

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file | 667 | 33 | **95.3%** |
| 0.95 | receiver-type match + same-package unique | 1,810 | 32 | **98.3%** |
| 0.9 | import qualifier, unique candidate | 1 | 0 | **100%** |
| 0.8 | global unique | 1 | 2 | 33.3% |
| 0.6 | demoted same-package multi-candidate | 93 | 249 | 27.2% |
| 0.5 | ambiguous global | 18 | 1 | 94.7% |

| Gate | Precision | Recall |
|------|----------:|-------:|
| `--min-confidence 0.7` (default) | **97.4%** | **83.5%** |
| `--min-confidence 0` (everything) | 89.1% | 87.3% |

### Receiver-type lite inference (v0.3.2)

The resolver now runs an **L0 receiver-type tier** before everything else:
the extractor infers the static type of call receivers (`x.M()` where `x` is
a method receiver, function parameter, or local declared with a literal /
`NewT()` constructor — no data-flow, no interface resolution), and method
definitions record their owner type (`symbols.scope` = Go receiver /
enclosing class). When exactly one file defines a same-named symbol owned by
the receiver's type, the ref resolves at 0.95.

Effect on gin (vs the 2026-06-10 demotion-era numbers): L0 resolves 482
calls; gated recall **79.1% → 83.5%** while precision *also* rose 97.2% →
97.4%. The recall the multi-candidate demotion had sacrificed is bought back
with type evidence instead of gambling. Remaining 0.6-tier errors (249) are
dominated by true interface dispatch — out of scope for name-based
resolution by design (an interface-typed receiver stays demoted; see
`tier0_unknown_receiver_stays_demoted`).

## History — 2026-06-10, v0.3.1-dev (demotion + GLOB fixes)

Gated 97.2% / 79.1%; everything 85.5% / 83.8%. Tier breakdown: same-file
866/35, same-pkg-unique 1,480/32, demoted-or-ambiguous 139/351.

### How the eval improved the resolver (same day)

The first run of this eval scored **85.6% gated precision**. Three fixes,
each verified by re-running the harness:

1. **Same-package multi-candidate demotion** — interface-method collisions
   (`Bind`×86, `String`×74, `Render`×37: many types in one package) were
   resolved at 0.95 by picking the first candidate. Now only *unique*
   same-package matches earn 0.95; collisions demote to 0.6, below the
   default gate. L2 precision: 80.7% → 97.9%.
2. **Qualifier-tier uniqueness** — build-tag variants (gin's `internal/json`
   has per-tag files all defining `Marshal`) made the qualifier tier gamble.
   Same treatment: unique candidate or demote.
3. **Case-sensitive qualifier matching** — SQLite `LIKE` is ASCII
   case-insensitive, so the *variable* `JSON` in `JSON.Bind()` matched the
   *file* `json.go` and the `encoding/json` import. Switched to `GLOB`.

Net: gated precision +11.6pp for −4.7pp recall — a caller list that is wrong
1-in-40 instead of 1-in-7. Demoted matches stay visible via
`possible_callers` / `--min-confidence 0`.

### Known weaknesses (current)

1. **Interface dispatch**: calls through interface-typed values are not
   resolvable by name + lite types — they stay demoted at 0.6 (249 of the
   ungated errors). Resolving them needs interface-satisfaction analysis,
   which is compiler territory; abyss surfaces them as `possible_callers`.
2. Remaining gated errors are same-file / receiver-tier name collisions
   (33 + 32 cases above) — mostly locals whose type the lite rules can't
   see (function returns, field accesses).
3. Coverage: TypeScript/Python/Java ground-truth runs are TODO
   (scip-typescript needs the corpus repo's node_modules; planned next).

## Corpus

| Repo | Ref | Language | Indexer |
|------|-----|----------|---------|
| gin-gonic/gin | v1.10.0 | Go | scip-go |
