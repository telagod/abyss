# Agent A/B pilot: does pre-edit caller context change agent behavior?

**Status: pilot (n=16). Published whatever the numbers say.**

## Question

When an AI agent edits a function with cross-file callers, does injecting
`abyss context` output (the pre-edit hook's payload) before the edit reduce
regressions or change how the agent works?

## Setup

- **Fixture**: a 6-package Go module (`eval/agent-ab/fixture`) with deliberate
  cross-file semantic contracts: integration tests in `report/` and `api/`
  pin user-visible behavior; unit tests pin leaf-module semantics.
- **Tasks** (4): change a leaf function's contract (cents→dollars, inclusive→
  exclusive bound, 1-based→0-based counter, insertion-order→sorted) such that
  exactly one caller in another package must compensate to keep the
  integration contract green.
- **Arms**: *control* gets the task only; *abyss* additionally gets the
  verbatim `abyss context <target>` output ("N external callers: file:line →
  caller()"). Same model (claude-haiku-4-5), same rules, 2 replicates.
- **Rules given to agents**: keep the system consistent end-to-end; the two
  integration test files must not be edited; no shell/build/test access
  (isolates static-context value from compiler feedback).
- **Grading**: `go test ./...` green + integration files byte-identical.

## Results (2026-06-10)

| Arm | Trials | Regressions | Tool calls | Wall-clock |
|-----|-------:|------------:|-----------:|-----------:|
| control | 8 | **0** | 112 | 287s |
| abyss | 8 | **0** | 98 (−12%) | 225s (−22%) |

### Honest reading

1. **Correctness hit a ceiling.** Both arms went 8/8. The fixture is small
   enough that a capable model finds every caller by reading 6 files, and the
   prompt's "keep the system consistent" requirement primes both arms to go
   looking. No correctness differentiation was measurable at this scale.
2. **The measurable effect was navigation efficiency**: the abyss arm used
   12% fewer tool calls and finished 22% faster at equal token budgets —
   the injected caller map replaced exploratory reading. Directionally
   consistent across t2/t3/t4; t1 was a wash (arms chose different but both
   valid adaptation strategies).
3. **What this pilot does NOT show**: that the hook prevents regressions in
   real codebases. That claim needs the next iteration.

---

# v2 — real corpus, placebo arm, no priming (2026-06-11)

**Status: run (valid-task n=6 per arm). Published whatever the numbers say.**

## Setup deltas from v1

- **Corpus**: gin v1.10.0 (102 files) — the SCIP eval corpus, not a toy fixture.
- **Tasks**: signature mutations on internal helpers with cross-file callers
  (`writeContentType []string→string`, 18 sites / 10 files; `assert1` param
  swap, 7 sites / 4 files). Phrased the way users ask ("change X to take Y") —
  **no** "keep the system consistent" priming.
- **Three arms**: *control* (task only), *grep* (task + raw
  `grep -rn "sym(" --include=*.go` dump — the placebo), *abyss* (task +
  verbatim `abyss context <target>`).
- **Agents**: claude-haiku-4-5, Read/Grep/Glob/Edit only — no shell, no
  compiler feedback. The grader is the compiler: miss one caller and
  `go test ./...` fails. Grading: signature actually changed + tests green +
  no `*_test.go` edited. Harness: `eval/agent-ab-v2/`.

## Results

| Arm | t1 (18 sites, 10 files) | t2 (7 sites, 4 files) | Valid-task total |
|-----|------------------------:|----------------------:|-----------------:|
| control | 3/4 | 2/2 | 5/6 |
| grep | **1/4** | 2/2 | 3/6 |
| abyss | **4/4** | 2/2 | **6/6** |

1. **abyss arm went 6/6.** Consistent ~23-turn runs on t1; every caller
   found, every replicate green.
2. **The placebo HURT: raw grep dumps were worse than nothing.** All three
   grep-arm t1 failures finished suspiciously fast (7 turns vs ~24) and
   edited only 3 of 10 files — the ones where a literal `[]string{...}` sat
   at the call site. The dump anchors the agent: it patches the lines it was
   handed and never follows variable declarations (`htmlContentType`) or
   reads surrounding types. Control agents, given nothing, explored more
   carefully and did better. *"Any context helps" is false — unstructured
   context invites premature confidence.*
3. t2 saturated (6/6 all arms) — 7 callers in 4 files is below the difficulty
   threshold where context matters, replicating v1's ceiling lesson at small
   scale.

## The harness audited the product (two real bugs)

Before producing a single comparison number, the v2 smoke trial caught two
silent-truncation bugs in `abyss context` itself:

1. The human-readable printer capped caller lists at 5 — header said
   "← 7 callers", listed 5, no ellipsis. The smoke-trial agent missed exactly
   the call site that fell off the list (`gin.go:339`).
2. One layer deeper, `find_callers_of` was queried with `LIMIT 20` and
   confidence-tied rows were cut arbitrarily — `debugPrint`'s 16 production
   callers surfaced as 14.

Both fixed (production callers now print in full; per-symbol cap 1000). An
agent-facing tool that silently truncates its safety contract is worse than
none — the A/B harness is also a regression test for that contract.

## Task-design lesson (excluded tasks)

Two of four tasks (t3 `nameOfFunction`, t4 `debugPrint`) turned out
**impossible by design**: those helpers are called directly from `*_test.go`,
so signature change + "don't edit tests" cannot compile — all arms 0/6 by
construction, excluded from analysis. Querying gin for internal helpers with
**zero direct test callers** and ≥2 cross-file production callers leaves
exactly two candidates (t1, t2): in a mature repo nearly every internal
helper is directly test-pinned. v3 task generation must filter on
`test_callers = 0` (the abyss index answers this in one SQL query) and will
need multiple corpora to assemble enough valid tasks.

## Honest limits

- Valid-task n=6 per arm, one model, one corpus; t1 is the only
  discriminating task. Directional, not definitive.
- Cost: 30 trials ≈ $4.6 (haiku).

## v3 backlog

Multiple corpora (hono/click already indexed) for ≥6 valid tasks; 2 models;
n≥5 per cell; add a "structured-but-stale" arm (outdated abyss context) to
measure the cost of trusting a wrong map.

## Reproduce

- v1: `eval/agent-ab/` (fixture + grade.sh)
- v2: `eval/agent-ab-v2/` — `run.sh [reps]` launches sandboxed headless
  trials (fresh gin copy each, `claude -p`, tool-restricted); `grade.sh`
  prints per-trial verdicts and per-arm summaries.
