# abyss design principles

The contracts abyss converged on across v0.3 → v0.5. Each one is pinned to
a concrete release or commit, not a slogan.

## 1. Precision over recall — the SCIP-eval contract

Agents act on what we tell them. A wrong answer at 0.95 confidence is more
expensive than no answer at all. Every resolver change is measured against
SCIP (compiler-grade) ground truth on six corpora across five languages.

- **Gate**: all corpora ≥ 98.5% gated precision. Below that is a
  release-blocker, no exceptions. Recall is reported but not gated.
- **Refresh policy**: when scip-* indexers move, baselines move in the
  same commit; pins live in `eval/setup-indexers.sh`. See
  [eval/notes/click-microregression-2026-06-17.md][click-micro] for the
  worked example (truth pairs 573 → 589 after a scip-python bump; no
  resolver regression, baseline updated).

## 2. Heuristic resolver, honest confidence

abyss is not a compiler. Tiered SQL resolution (L0 → L6) emits a
confidence score per ref; agents see a default `min_confidence=0.7` gate
and any number below 0.95 is, by construction, a guess we're willing to
defend.

- **Tier thresholds were chosen by measurement**, not aesthetics — each
  level's confidence is whatever precision it scored against SCIP on the
  six corpora.
- **Confidence is a contract.** Changing a tier threshold requires
  re-running eval; the threshold lives in code, the score lives in
  `eval/RESULTS.md`, the two must agree.
- **When a hypothesis fails, log it.** Python MRO walker (L0e) shipped
  with a prediction: "≥50 hits on FastAPI". It fired 0 times. Documented
  in [docs/dogfood/fastapi-2026-06-17.md][fastapi] and the v0.5.0
  CHANGELOG instead of buried.

## 3. Ambient delivery over MCP-tool-fetch

An agent that has to *ask* for context will skip it. The v0.4.0 lesson:
the pre-edit hook is the right delivery channel for `<abyss-card>`,
because it fires on every edit and costs nothing the agent has to
choose. The v0.5.0 lesson: the watcher belongs in the background.

- **Hook is read-only and < 30 ms.** v0.3.6 → v0.4.0 dropped p99 from
  1072 ms (sync re-index trap) to ~11 ms by moving refresh to
  `post-edit`. 96× faster on the worst case.
- **Hooks must never block the agent.** Every error path in `cmd_hook`
  is silent success. No panics, no stderr noise except actionable
  warnings.
- **Daemon is the warm path.** v0.5.0's `abyss daemon start --detach`
  (double-fork + setsid + pidfile claim before parent returns) keeps the
  index hot without the user remembering to run `abyss watch` in a
  terminal.

## 4. Dogfood reveals truth tests don't

Eval against SCIP measures one axis (precision/recall on resolved
refs). It does not measure whether the output is *useful* to an agent
reading it. Running abyss on real third-party codebases does.

- **helix-editor** ([report][helix]) — found the contracts-section
  duplication (`default (method)` × 18 in `editor.rs`) and the
  Rust-workspace labeller blindness (`helix--N` placeholders).
- **vite** ([report][vite]) — found the **single biggest TS gap**:
  `callers ViteDevServer` returned 0 because the resolver was filtering
  to `kind='call'` only. 75 high-confidence `type_ref` rows existed and
  were invisible. v0.5.0 makes both kinds the default.
- **FastAPI** ([report][fastapi]) — found that the F4 test-path filter
  (`%/tests/%`) missed top-level `tests/` directories, and that
  `docs_src/` tutorials were leaking into ambiguous picks.
- **Rule**: every dogfood ships with a final score, a list of bugs
  surfaced, and at least one fix in the next release that cites the
  report. Score `7/10` is not a participation trophy — it means we
  found four things to fix.

## 5. Honest module labels: `cluster-N` over made-up names

Module labels go to humans and to agents. A label that *looks*
informative but isn't (e.g., `helix--8`, `p-18`, `mixed:create-vite+`)
costs trust. The v0.5.0 labeller learned three rules:

- Strip boring monorepo prefixes (`packages`, `crates`, `libs`, `src`,
  `lib`, `app`, `apps`, `services`, `modules`, `internal`) before picking
  the modal segment — turns `p-18` into `vite-18`.
- Cross-cutting communities (no single segment ≥ 50% of members)
  render as `cluster-N`, not `mixed:{seg}+`. The user can tell at a
  glance "we couldn't name this" instead of decoding internal
  cluster-merge state.
- Genuinely unlabelled modules render as `unlabelled-N`.

Principle: **if the heuristic can't produce a human-meaningful label,
say so**. Don't fabricate one out of a fragment.

## 6. Bounded resource use is part of correctness

A 102 MB index on a 1953-file Go backend is a bug, not a feature.
Bounding is enforced at three layers:

- **Workspace safety** — refuse `$HOME` / `/`; circuit-break on a
  large unversioned directory (no `.git`).
- **Bounded temporal mining** — `commit_files` keeps only indexed-file
  paths; coupling excludes bulk commits > 50 files.
- **Codegen-aware indexing** — files marked `DO NOT EDIT` / `@generated`
  keep symbols + chunks but skip ref extraction unless
  `--index-generated`.

Measured impact (v0.3.6 → v0.5.0 baseline): 1953-file Go backend dropped
102 MB → 75 MB, refs −34%, coupling −90%.

## 7. Zero-regression eval gate

A release blocks if any of these fail:

1. **`cargo fmt --check` + `cargo clippy -D warnings`** on both `slim`
   and `--features semantic`.
2. **`cargo test`** — all integration + unit tests pass.
3. **SCIP eval** — every corpus stays at or above its current baseline
   (gated precision). Recall regressions get reported but do not block.
4. **Schema migration** — additive only (`CREATE TABLE IF NOT EXISTS`
   / `ALTER TABLE ADD COLUMN`); existing indexes stay forward-compatible.

When a release does no resolver work — like v0.5.0 — the eval gate
passes by definition (no change to the scoring axis). We still publish
the table to make zero regression a visible commitment.

[click-micro]: ../eval/notes/click-microregression-2026-06-17.md
[fastapi]: dogfood/fastapi-2026-06-17.md
[helix]: dogfood/helix-editor-2026-06-17.md
[vite]: dogfood/vite-2026-06-17.md
