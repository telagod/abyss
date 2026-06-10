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

## Next iteration (designed, not yet run)

- Corpus: real repos (100+ files) where callers are not trivially greppable —
  use the SCIP eval corpus (gin et al.) with mutation-derived tasks.
- Remove the "keep the system consistent" priming: phrase tasks the way users
  actually do ("change X to do Y"), so caller-awareness must come from the
  agent or the hook, not the prompt.
- Larger n, multiple models, and a third arm (grep-hint placebo) to separate
  "any context helps" from "caller context helps".

## Reproduce

Fixture and grading: `eval/agent-ab/`. Trials were run as sandboxed agent
sessions, one fresh fixture copy per trial; grade with
`eval/agent-ab/grade.sh` (go test + integration-file integrity).
