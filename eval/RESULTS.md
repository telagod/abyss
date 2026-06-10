# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-10, abyss v0.3.1-dev (after eval-driven fixes)

### gin v1.10.0 (Go, 102 files) — ground truth: scip-go

2,968 ground-truth call pairs · abyss index time: **146ms** (scip-go: ~40s)

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file | 866 | 35 | **96.1%** |
| 0.95 | same package, unique candidate | 1,480 | 32 | **97.9%** |
| 0.9 | import qualifier, unique candidate | 1 | 0 | **100%** |
| 0.8 | global unique | 1 | 2 | 33.3% |
| 0.6 / 0.5 | demoted multi-candidate / ambiguous | 139 | 351 | 28.4% |

| Gate | Precision | Recall |
|------|----------:|-------:|
| `--min-confidence 0.7` (default) | **97.2%** | **79.1%** |
| `--min-confidence 0` (everything) | 85.5% | 83.8% |

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

1. **Interface dispatch**: calls through interface-typed values resolve to a
   concrete impl file, while SCIP's truth is the interface declaration —
   inherently beyond name-based resolution. Receiver-type inference (lite)
   is the next lever.
2. Remaining gated errors are same-file / same-package name reuse on
   different types (35 + 32 cases above).
3. Coverage: TypeScript/Python/Java ground-truth runs are TODO
   (scip-typescript needs the corpus repo's node_modules; planned next).

## Corpus

| Repo | Ref | Language | Indexer |
|------|-----|----------|---------|
| gin-gonic/gin | v1.10.0 | Go | scip-go |
