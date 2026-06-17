# Dogfood index

abyss claims to make agents faster on real codebases. We test the claim
by running it on real codebases and publishing the result — score,
bugs found, falsified predictions — under `docs/dogfood/`.

Every report follows the same shape: index stats, per-probe results
(`where` / `context` / `callers` / `impact` / pre-edit card), per-axis
score, and a bug list that drives the next release's UX fixes.

## Final scores

| Project | Date | Language | Files | Cold index | Score | Report |
|---------|------|----------|------:|-----------:|------:|--------|
| helix-editor @ `43bf7c2` | 2026-06-17 | Rust workspace (~243 .rs) | 545 | 1.57 s | **7.5 / 10** | [helix-editor-2026-06-17.md](dogfood/helix-editor-2026-06-17.md) |
| vite v5.4.0 | 2026-06-17 | TS / JS monorepo | 1 793 | 0.91 s | **7 / 10** | [vite-2026-06-17.md](dogfood/vite-2026-06-17.md) |
| FastAPI 0.115.4 | 2026-06-17 | Python | 2 164 | 1.07 s | **6.5 / 10** | [fastapi-2026-06-17.md](dogfood/fastapi-2026-06-17.md) |
| hono v4.6.14 | 2026-06-17 | TypeScript | 388 | 0.79 s | **8 / 10** | [hono-2026-06-17.md](dogfood/hono-2026-06-17.md) |

Scores are calibrated against three axes per probe (signal density,
noise, latency) and one overall number. `7/10` means "load-bearing
right now, two visible gaps"; `6.5` means "still useful, one falsified
hypothesis".

## Per-project summary

### helix-editor (Rust workspace, 100K LOC)

abyss handles a 545-file / 5.6K-symbol Rust workspace in **1.5 s
cold / 360 ms warm**, p95 hook latency 22 ms. Topology signals (in/out
degree, centrality, role) are honest and useful — top-10 centrality is
the right set of "if you change these you break the editor" files
(`selection.rs`, `rope.rs`, `transaction.rs`, `document.rs`). Card
delivers 937 prod callers across 45 files for `editor.rs` — best-in-
class blast radius. The 2.5 points lost are all presentation:
Rust-workspace labels collapse to `helix--N` placeholders, the layer
dictionary is web/service-tuned and gives 83% of files `unknown`, and
the contracts section repeats trait methods (`default` × 18) because
they collide on bare name. Read the full report:
[dogfood/helix-editor-2026-06-17.md](dogfood/helix-editor-2026-06-17.md).

### vite (TS/JS monorepo, 1793 files)

abyss handles a 1793-file polyglot TS/JS monorepo in **863 ms cold /
172 ms warm**, p95 hook latency under 20 ms, zero cross-language-family
edges in 28K refs. The cross-language guard holds clean. `context
resolve.ts` names every external entry point and where they're called;
`impact resolveConfig` returns 4 direct + 30 transitive callers + risk
7.4/10 in 17 ms — exactly the agent-blocking pre-edit answer abyss
exists to deliver. **Headline finding**: `callers ViteDevServer`
returned 0 because the resolver was filtering to `kind='call'` only —
75 high-confidence `type_ref` rows existed and were invisible. This
finding directly drove v0.5.0's default-both behavior on `callers`.
Secondary gaps: monorepo labels (`p-18`, `mixed:create-vite+`), L4
mis-resolving `debug` to a test fixture, contracts duplication. Read:
[dogfood/vite-2026-06-17.md](dogfood/vite-2026-06-17.md).

### FastAPI (Python, 2164 files)

Run as the validation case for Python L0e — the MRO-aware receiver
walker we predicted would fire ≥50 times on FastAPI. **It fired 0
times**. Root cause is structural: FastAPI's class hierarchy roots
(`Starlette`, `BaseModel`, `Request`, `Enum`) all live in external
libraries, and L0e by design only walks in-repo inheritance chains.
The prediction was falsified, written down, and CHANGELOG'd — see
PRINCIPLES.md section 2 for why we keep this kind of negative result
visible. Useful side findings: F4's `%/tests/%` filter misses
top-level `tests/`, `docs_src/` tutorials leak into ambiguous picks,
arch-labeller pulls hub files into satellite clusters by connectivity.
Card body for `applications.py` (1889 bytes) is genuinely
agent-ready — 2021 callers across 734 files with HIGH BLAST RADIUS
warning, contracts dedup nailed. Read:
[dogfood/fastapi-2026-06-17.md](dogfood/fastapi-2026-06-17.md).

### hono (TypeScript, v4.6.14)

abyss indexes hono's 388-file TypeScript framework in **793 ms cold /
333 ms warm**, p95 probe latency under 25 ms. This was the first
dogfood we ran (the W1 UX debts it surfaced fed v0.4.0), and the
detailed report was backfilled post-v0.5.0 so the deltas v0.4.0 +
v0.5.0 closed could be measured against the same workload. **Three
v0.5.0 wins are directly observable**: (1) `callers Context` now
returns 20 type-position callers annotated `(100%, type)` — under
v0.4.0 the same command returned zero, the vite-report's headline
finding; (2) the pre-edit card for `src/context.ts` surfaces
`151 prod callers (46 call, 105 type) across 43 files` — call/type
split is first-class; (3) search for "middleware" returns zero
`*.test.ts` files in the top 10 thanks to L4 test-skip. Remaining
debts: `callers` silently caps at 20 rows when 235 exist, `Set`
interface collides with the JS built-in, `module=middleware-10`
labels still aren't human-meaningful. Read the full report:
[dogfood/hono-2026-06-17.md](dogfood/hono-2026-06-17.md).

## What dogfood taught us

- **`kind='call'` was a silent default that broke TypeScript.** No
  amount of unit-test coverage on the resolver surfaced this — only
  asking the live question "who uses `ViteDevServer`?" did. Default
  behaviors that look uncontroversial in code review get caught at
  dogfood time.
- **Module labels were the second-largest UX gap.** Three of four
  reports flagged the same thing (`helix--N`, `p-N`, `mixed:create-vite+`,
  `cluster-2` swallowing `routing.py` because tests dominate the cluster).
  The fix isn't smarter NLP — it's honest fallbacks (`cluster-N`,
  `unlabelled-N`) plus monorepo-prefix stripping.
- **Test-path filters need to be anchored, not just substring-matched.**
  FastAPI's top-level `tests/` slipped past `%/tests/%` — a 1-line SQL
  fix worth a regression test.
- **Predictions get falsified more often than we'd like to admit.**
  L0e was sold on FastAPI; FastAPI delivered 0 hits. The honest
  response was to log the falsification (PRINCIPLES.md §2,
  [eval/notes/click-mro-walker-2026-06-17.md](../eval/notes/click-mro-walker-2026-06-17.md))
  and pick a better corpus next time (Django ORM, SQLAlchemy core —
  somewhere the hierarchy stays in-repo). Don't quietly bury the bad
  number.
- **The pre-edit card is the load-bearing artifact.** Across all four
  dogfoods, the highest-rated probe was the card itself — `where` +
  `depended-on` + `contracts` + `recent activity`, delivered ambiently
  by the hook. That's what makes abyss feel useful instead of feeling
  like another search tool.
- **Cold index < 2 s on every real-world target.** helix (1.5 s),
  vite (0.9 s), FastAPI (1.07 s). The hash-incremental warm path
  stays under 400 ms even on the largest. The "<5 s on medium
  codebases" budget held with room to spare.

## Reproducing a dogfood

```sh
git clone --depth 1 <target-repo> /tmp/<name> && cd /tmp/<name>
abyss index
abyss stats
abyss map
abyss where <hub-file>
abyss context <hub-file>
abyss callers <core-symbol>
abyss impact <core-symbol>
echo '{"tool":"Edit","args":{"file_path":"<hub-file>"}}' | abyss hook pre-edit
```

Then write it up against the four-probe shape (`where` / `context` /
`callers` / `impact`) with per-axis scores 0–5 (signal density / noise /
latency) and a final score 0–10. Bugs go in a numbered list at the end
so the next release can cite them by name.
