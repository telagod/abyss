# Eval: abyss call-graph resolution vs SCIP ground truth

Published whatever the numbers say. Reproduce with `eval/run.sh`.

## Method

For every call reference abyss extracts, SCIP (compiler-grade indexing) tells
us where the called symbol is actually defined. abyss's prediction is correct
iff it resolved the call to the same file. Join key: `(file, line, symbol name)`;
only in-repo symbols count (abyss does not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it right
- **recall** — how much of the SCIP-known call graph abyss resolves correctly

## Results — 2026-06-10, abyss v0.3.0

### gin v1.10.0 (Go, 102 files) — ground truth: scip-go

2,968 ground-truth call pairs · abyss index time: **146ms** (scip-go: ~40s)

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file | 866 | 35 | **96.1%** |
| 0.95 | same package | 1,601 | 382 | **80.7%** |
| 0.9 | import qualifier | 19 | 0 | **100%** |
| 0.8 | global unique | 1 | 2 | 33.3% |
| 0.5 | ambiguous guess | 0 | 1 | 0% |

| Gate | Precision | Recall |
|------|----------:|-------:|
| `--min-confidence 0.7` (default) | **85.6%** | **83.8%** |
| `--min-confidence 0` (everything) | 85.5% | 83.8% |

### Known weaknesses (from this eval)

1. **Same-package method-name collisions dominate the errors.** gin defines
   `Bind`/`String`/`Render`/`WriteContentType` on many types in one package;
   the same-package tier picks one candidate file. Top offenders: `Bind` (86),
   `String` (74), `Use` (55), `Render` (37). Fix direction: demote
   multi-candidate same-package matches to a lower confidence tier, and use
   receiver hints where available.
2. The 0.8/0.5 tiers carry almost no volume in a single-module Go repo —
   their numbers here are anecdotal, not significant.
3. Coverage: TypeScript/Python/Java ground-truth runs are TODO
   (scip-typescript needs the corpus repo's node_modules; planned next).

## Corpus

| Repo | Ref | Language | Indexer |
|------|-----|----------|---------|
| gin-gonic/gin | v1.10.0 | Go | scip-go |
