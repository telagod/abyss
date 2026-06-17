# Release notes — v0.5.x

> One-page overview of the v0.5.x series. Per-version detail is in
> [CHANGELOG.md](CHANGELOG.md); this file is the "what was v0.5.x
> *about*?" capstone.

abyss v0.5.0 shipped on 2026-06-17 as the **"agent always has the map
(in the background)"** release. The 17 small patches that followed —
v0.5.1 → v0.5.20 — did not break any API or schema. They came out of
a dogfood-driven loop: run abyss on a real codebase, pick the most
visible debt, fix it, ship, repeat. Six dogfood reports later, the
series ends with a daemon that subscribes, a Python coverage matrix
that pins MRO behaviour by regression test, and an operability
surface that finally feels finished.

## Themes

The 17 patches between v0.5.0 and v0.5.20 group into four storylines.

### V2 daemon emerges

The v0.4.0 daemon was a watcher with two socket verbs (`ping`,
`stats`). v0.5.x turned it into the warm path:

- **v0.5.2** — `abyss mcp --via-daemon` tunnels the agent's stdio
  through a running daemon, so multiple MCP clients share one
  warm index (per-connection read-only WAL handles).
- **v0.5.7** — `abyss daemon logs --follow` streams the tail
  continuously, so an agent (or operator) watching reindex activity
  doesn't have to poll.
- **v0.5.16** — socket verb `subscribe` push-notifies long-lived
  agents on every reindex `epoch` bump. The agent stops re-asking
  "did the index change?" and starts being told.

The pre-edit hook deliberately stayed on the direct-SQLite path
throughout — a socket hop would have regressed its sub-12 ms budget
for no agent-visible benefit. v0.5.5's hostile-concurrent bench
(50 parallel hooks against one DB) measured **p99 = 82 ms with zero
failures**, validating the no-daemon hot path.

### Dogfood-driven UX

Every UX patch in v0.5.x was traceable to a specific dogfood finding:

- **v0.5.0** — `callers` defaults to call+type+inherit because vite's
  `callers ViteDevServer` had returned 0 (type-position refs were
  silently filtered out). Type-position refs annotate with `(type,
  95%)` so the kind is first-class in the output.
- **v0.5.1** — `callers --limit N` + non-silent `(showing N of M)`
  footer from hono B1 (`callers Context` truncated 215 of 235 rows
  with no indication). Layer-dictionary expansion for framework
  entry filenames (`hono.ts`, `vite.ts`, `manage.py`, …).
- **v0.5.2** — small-cluster module labels switch to
  peak-centrality tiebreaker when the Louvain community has <8
  members. Fixes FastAPI's `param_functions.py` mis-labelling
  as `docs_src`. TSX visitor extracts JSX refs.
- **v0.5.4** — Rust and TS built-in symbol filters so `callers Set`
  doesn't claim the JS global, `callers Vec` doesn't claim the Rust
  std type.

### Python coverage matures

Python was the most-exercised language family in dogfood (FastAPI,
Django, SQLAlchemy — three runs out of six). Each surfaced a gap that
got pinned:

- **v0.5.2** — generic-base inherit edges (`class Sub(Base[T]):`
  used to drop via a `_ => continue` arm). SQLAlchemy
  `callers ColumnElement` jumped from 13 to 35 with the fix.
- **v0.5.3** — first-class `type_ref` emission for typed parameters,
  returns, and locals. Pre-fix, these positions only fed
  receiver-inference but never became graph edges — cross-language
  asymmetry surfaced by the docs-bundle agent.
- **v0.5.13** — `.pyi` stub files indexed end-to-end (parse + chunk
  + ref-extract + import-binding resolver candidates).
- **v0.5.17** — same-file MRO walk pinned by regression test
  (Audit note: the L0e walker MUST traverse a co-located `Base ←
  Sub` inherit edge; a future refactor adding `WHERE f.id != ?` for
  some other reason would now surface here).

The series tops out with **SQLAlchemy L0e firing 4 496 times** on a
declarative-base hierarchy (9× the floor predicted from Django).

### Operability

The shell+config+CLI surface picked up the table-stakes commands
agents and operators kept hand-rolling:

- **v0.5.8** — `abyss completion {bash,zsh,fish,powershell}` for
  shell tab-completion across all subcommands.
- **v0.5.10** — `abyss config show` emits the effective merged
  config (defaults + workspace + env), no more guessing what
  `.code-abyss/config.toml` actually does.
- **v0.5.11** — `abyss reset` for wipe-and-rebuild from a clean
  state without nuking the workspace.
- **v0.5.12** — `abyss index --since <ref>` reindexes only files
  changed since a git ref, for CI incrementals.

## Bench numbers

The v0.5.x story validated three claims the v0.4.0 release made:

- **Hook p99 ≈ 11 ms** on the standard burst-edit bench (hostile
  workload, 100 file batches). Unchanged from v0.4.0 — the
  V2-daemon work did not touch the hot path.
- **Hostile-concurrent p99 = 82 ms with zero failures** (v0.5.5
  bench, 50 parallel hooks against one DB).
- **L0e MRO walker fires 4 496 times on SQLAlchemy 2.0.36** (687
  Python files, 291 refs/file — densest Python corpus measured),
  **9 450 times on Django 5.1.4** (3 292 files), **0 times on
  FastAPI** (external roots — Starlette, pydantic — falsified the
  ≥50-hit prediction in public).

SCIP gated precision held across all six eval corpora through the
whole v0.5.x series — zero resolver-tier behaviour changes affected
the scoring axis.

## Test count

| | v0.5.0 | v0.5.1 | v0.5.20 |
|---|------:|------:|------:|
| Integration tests | 295 | 307 | **386** |

The +79 in v0.5.x is overwhelmingly regression tests pinning a
behaviour a dogfood surfaced — not new functionality. The contract
v0.5.x converged on: **every dogfood finding gets a test before the
fix lands.**

## What v0.5.x didn't solve

Three open items roll forward to v0.6 / V2.1:

- **`impact direct=N` vs `callers prod=N` semantic gap.** The two
  commands apply different definitions of "direct caller" (impact
  filters to refs targeting the canonical-file declaration;
  callers includes refs to any same-named symbol). Unification
  needs a shared `caller_set(sym, opts)` primitive — a V2 item.
- **Centrality-weighted module labelling still drowns hub files in
  tiny clusters.** `where django/db/models/base.py` returns
  `module=tests-27` because 1 800+ test files inherit from `Model`
  and bidirectional clustering drags the hub into the satellite.
  Fix path: weight `inherit` / `type_ref` edges differently from
  `call` in the Louvain pass, or prefer source-directory prefix as
  a tiebreaker.
- **TSX un-resolution rate still higher than `.ts`** on hono
  (66.8 % vs 58.9 %) despite the v0.5.2 JSX visitor. Component
  refs need their own L0-tier binding pass.

## Quick start / Where to go next

- **Install + first index**: [README.md](README.md#quickstart-60-seconds)
- **Full docs**: [telagod.github.io/abyss](https://telagod.github.io/abyss/)
- **Dogfood reports**: [docs/DOGFOOD.md](docs/DOGFOOD.md) +
  [docs/dogfood/*.md](docs/dogfood/) — six honest field reports
  with bugs found and falsified predictions in public.
- **Per-version detail**: [CHANGELOG.md](CHANGELOG.md)
- **Design contracts**: [docs/PRINCIPLES.md](docs/PRINCIPLES.md)
