# Design principles

The opinionated decisions abyss is built on. These are the
non-negotiables — every PR that touches them needs a
documented reason.

## No language server dependency

Resolution is heuristic (tree-sitter + SQL tiers), not compiler-grade.
Trade-off: faster and simpler, but confidence scores must be honest.

abyss indexed gin in **~150 ms**; scip-go took ~40 s. The cost is a
gated recall ceiling that no one-pass heuristic can clear — interface
dispatch, dynamic dispatch, metaprogrammed methods. Those stay
demoted by design, surfaced as `possible_callers`.

## Confidence is a contract

Every ref carries a confidence score stored in the DB. Agent-facing
APIs default to `min_confidence=0.7` to filter noise. Changes to
confidence thresholds require eval validation against every SCIP
corpus — regressions there are release-blockers.

## Hash-incremental indexing

Only re-index files whose blake3 hash changed. The pipeline is
designed to run in <5 s on medium codebases. The first index is the
expensive one; everything after is hash diffs.

## Hooks must never block the agent

`cmd_hook` silently succeeds on every error path — no panics, no
stderr noise except actionable warnings. If abyss can't read the
file or the DB is locked, the agent gets a clean `exit 0` and the
edit proceeds. The card is best-effort context, not a gate.

## Bounded resource use

Three guards keep the index proportional to hand-written code, not
to repo size:

1. **Workspace safety** — refuse `$HOME` / `/`, file-count circuit
   breaker without `.git`.
2. **Bounded temporal mining** — `commit_files` keeps only
   indexed-file paths; coupling excludes bulk commits >50 files.
3. **Codegen-aware indexing** — generated files skip ref extraction
   unless `--index-generated`.

Measured: a 1953-file Go backend's index dropped 102 MB → 75 MB,
refs −34%, coupling −90%.

## Single binary, single SQLite file

No daemons by default. No external services. No model downloads in
the slim build. One `abyss` binary, one `.code-abyss/index.db` per
workspace. Add `--features semantic` only when you need
embedding-based search.

## Published whatever the numbers say

`eval/RESULTS.md` is the source of truth for resolver accuracy. Every
release re-runs the SCIP eval; the numbers ship unredacted, including
the ones that look bad. See [Per-corpus results](../eval/results.md).
