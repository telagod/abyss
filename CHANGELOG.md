# Changelog

## v0.5.0 — 2026-06-17

The **"agent always has the map (in the background)"** release. 45 commits
since v0.4.0, four lines of work:

1. **Daemon goes background**. `abyss daemon start --detach` does a proper
   double-fork; the watcher process survives the shell that launched it.
   New socket verbs `reindex` and `logs` plus an `abyss daemon logs` CLI
   make the daemon operable from outside its own log file.

2. **`callers` learns to follow type references** (highest-ROI debt
   surfaced by the vite dogfood). `callers ViteDevServer` went from 0 to
   71 prod users — TS interfaces, generics, and `extends` clauses now
   surface as first-class callers. New `--calls-only` / `--types-only`
   flags. MCP `find_callers` gains a `kinds` filter.

3. **Three honest dogfood reports**. We ran abyss on hono (7/10), helix-
   editor (7.5/10), vite (7/10), FastAPI (6.5/10) and committed the
   reports as `docs/dogfood/*.md`. They surface real debts and one
   falsified hypothesis: MRO L0e doesn't fire on FastAPI because its
   class hierarchy roots are in external libraries (Starlette, pydantic).
   We log the prediction error so future MRO discussion stays honest.

4. **UX polish driven by what dogfood found**. Module labels
   (`p-18` → `vite-18`, `mixed:temporal+` → `cluster-1`), contracts
   dedup (helix's `default × 18 impls` becomes one row), L4 stops
   resolving common names to test fixtures, `map` empty-hotspots
   explains itself.

### New commands

- `abyss daemon start --detach` — double-forked background daemon.
- `abyss daemon logs [--tail N]` — tail the daemon log via socket or
  direct file read.
- `abyss callers <sym> --calls-only / --types-only` — restrict caller
  results by ref kind. Default is "both" (the v0.4.x behavior was
  silently `--calls-only`).
- `abyss daemon start [--foreground]` / `daemon stop` / `daemon status` —
  background daemon V1 (Unix only). The watcher moves out of the foreground
  process: a single-instance pidfile lock (flock) under
  `.code-abyss/daemon.pid` and a Unix socket at `.code-abyss/daemon.sock`
  serve a minimal newline-delimited JSON protocol. Two verbs in V1:
  `{"cmd":"ping"}` returns `uptime_secs` + `last_reindex_ms` + `epoch`,
  `{"cmd":"stats"}` returns file/symbol/ref counts. SIGTERM/SIGINT trigger a
  clean shutdown that unlinks the pidfile and the socket. `abyss watch`
  keeps working unchanged as the foreground alias (equivalent to
  `daemon start --foreground`). V2 (full MCP-over-socket multi-reader) is on
  the roadmap; the V1 pre-edit hook still reads SQLite directly — no daemon
  round-trip on the fast path.

### Daemon V1.5

- `abyss daemon start --detach` — proper double-fork + `setsid` so the
  daemon survives the shell that launched it without `&`. Stdin is closed,
  stdout/stderr land in `.code-abyss/daemon.log`. The parent process
  returns once the grandchild has claimed the pidfile (≤500ms), so
  `daemon start --detach && daemon status` chains cleanly.
- `abyss daemon logs [--tail N]` (default `N=50`) — tail the daemon log.
  Goes through the socket's new `{"cmd":"logs","tail":N}` verb when the
  daemon is live; falls back to a direct file read when it's not.
- Socket verb `{"cmd":"reindex"}` — synchronous hash-incremental
  `IndexPipeline::run_structural` on a worker thread. Returns
  `{ok, reindexed, removed, duration_ms, epoch}`. Operator-triggered
  reindexes are serialized against each other via a `try_lock`'d mutex;
  the loser gets `{"ok":false,"error":"index lock contention"}` rather
  than queueing silently.
- Socket verb `{"cmd":"logs","tail":N}` — last N lines of `daemon.log`,
  streamed via a bounded `VecDeque` so memory stays O(N) regardless of
  log size.

### Added

- Python L0e tier — MRO-aware receiver inference walks `inherits` refs from
  `class Sub(Base):` declarations up the chain (6-hop cap, single-DFS
  approximation of C3). When the receiver's class doesn't define the
  called method but a base does, the call resolves to the base's file at
  confidence 0.95. Click eval: 2 hits (hierarchy co-locates).
  FastAPI eval: **0 hits** (hierarchy roots are external libraries —
  Starlette / pydantic). Honest falsification logged in
  [eval/notes/click-mro-walker-2026-06-17.md](eval/notes/click-mro-walker-2026-06-17.md).
- 4 dogfood reports: hono (7/10), helix-editor (7.5/10), vite (7/10),
  FastAPI (6.5/10). All under `docs/dogfood/`.
- `eval/setup-indexers.sh` — one-shot reproducible install of all five SCIP
  indexers (scip / scip-go / scip-typescript / scip-python / scip-clang)
  with pinned versions. `eval/README.md` documents the policy: bumping a
  pinned indexer requires updating baselines in the same commit.
- Pre-existing `arch_facts` table now includes `inherits` ref edges from
  Python class declarations.

### Changed

- **Module labels** got honest. Common-prefix paths used to collapse to
  one letter + module id (`packages/` → `p-18`, `helix-view/` →
  `helix--8`); the labeller now treats `packages` / `crates` / `libs` /
  `src` / `lib` / `app` / `apps` / `services` / `modules` / `internal` as
  boring prefixes and picks the modal next segment (`vite-18`,
  `helix-view`). Cross-cutting clusters render as `cluster-{id}` instead
  of leaking internal cluster-merge state (`mixed:{seg}+`).
- **Contracts dedup**: `render_card` groups by `(name, kind)` and adds
  `(N impls)` when multiple definitions share the same symbol. Helix's
  card no longer shows `default (method) → 35 callers` eighteen times;
  it shows one row tagged `(18 impls)`.
- **`abyss callers` defaults to both kinds**. Old behavior — `kind='call'`
  only — silently returned zero results for exported TS interfaces and
  Rust types. New default includes `kind='type_ref'`. Output suffixes
  each caller with `(call, 95%)` or `(type, 95%)` when results mix kinds.
  The card's `depended-on` section likewise reflects both.
- **`abyss watch`** keeps working unchanged; it's now an alias of
  `abyss daemon start --foreground`. Use `--detach` for V1.5 background
  semantics.
- L4 + L4b resolver tiers now exclude test-path TARGETS from candidate
  picks (patterns: `__tests__/`, `_test.`, `.test.`, `.spec.`, `/test/`,
  `/tests/`, `/playground/`). Source-side unchanged — refs FROM a test
  file still resolve to real impls. Fixes vite's `debug` import getting
  silently routed to `playground/hmr-ssr/__tests__/hmr-ssr.spec.ts`.
- `abyss map` explains empty hotspot lists — `(insufficient history: N
  commits available; need ≥10)` for shallow clones,
  `(no files changed in the last 30 days)` for historical repos.
- SCIP indexer versions pinned in `setup-indexers.sh`; `run.sh` logs
  `--version` of each on every run so baselines stay reproducible.

### Fixed

- `pipeline.reindex_file` no longer silently CASCADE-deletes the touched
  file's outgoing refs. The function used to call `repo.delete_file()`
  before re-inserting, which triggered `ON DELETE CASCADE` on
  `refs.source_file_id` and nuked the file's outgoing edges. Now
  delegates to `run_structural` (hash-incremental, runs the full batch
  resolver). The watcher V1 already routed around it; this closes the
  latent bug for any future caller. Verified via
  `tests/reindex_file_preserves_refs.rs`.

### Eval — gated precision / recall vs SCIP ground truth (v0.5.0)

| Corpus | Lang | v0.4.0 | v0.5.0 | Δ |
|--------|------|--------|--------|---|
| gin v1.10.0 | Go | 99.3 / 82.6 | **99.3 / 82.6** | 0 / 0 |
| hono v4.6.14 | TypeScript | 98.8 / 63.8 | **98.8 / 63.8** | 0 / 0 |
| click 8.1.8 | Python | 97.9 / 93.0 | **97.9 / 93.0** | 0 / 0 |
| ripgrep 14.1.1 | Rust | 98.5 / 75.3 | **98.5 / 75.3** | 0 / 0 |
| abyss dogfood | Rust | 100.0 / 90.9 | **100.0 / 90.9** | 0 / 0 |
| cmark 0.31.1 | C | 99.1 / 74.8 | **99.1 / 74.8** | 0 / 0 |

Six corpora zero regression. v0.5.0 work was UX-and-operability focused —
no resolver tier changes affecting the SCIP-ground-truth scoring axis.

### Schema

No schema bumps in v0.5.0. v6 (added in v0.4.0) is the current version.

### Roadmap items unblocked

- V2 daemon (full MCP-over-socket multi-reader) — V1.5's reindex/logs verbs
  are the first step.
- Python MRO retest on Django / SQLAlchemy — FastAPI falsified the "≥50
  hits" prediction; the gap was in-repo inheritance depth, not codebase
  size. Need a corpus where the hierarchy lives in-repo.

---

## v0.4.0 — 2026-06-17

The "agent always has the map" release. 39 commits since v0.3.6, organized
around three lines:

1. **Ambient delivery actually works**. The pre-edit hook is now read-only
   and renders a structured `<abyss-card>` system-reminder block — hook p99
   went from **1072 ms** (sync re-index trap) to **~11 ms** on the hostile
   burst-edit bench. 96× faster on the worst case.

2. **L0 architectural coordinates**. Every indexed file gets a layer / role /
   module / centrality / depth-from-entry tuple, inferred from a directory
   regex dictionary + entry-point detection + naming patterns +
   PageRank/SCC/Louvain on the import graph. The card's `where` line is no
   longer "unknown".

3. **Dogfood-driven UX cleanup**. Three live debts surfaced while running
   abyss on hono and got fixed: path matching now prefers shortest exact
   path, search ranks impl above test/import via centrality + penalty, and
   `abyss callers` excludes tests by default.

### New commands

- `abyss watch` — foreground daemon-lite that subscribes to file-system
  events and reindexes incrementally on save (150 ms debounce by default).
- `abyss where <file>` — show this file's coordinates in the codebase:
  layer, module, role, centrality, in/out degree.
- `abyss attach claude [--local]` — idempotently install the hook into
  `~/.claude/settings.json` (or workspace-local).
- New MCP tool `arch_map` exposing layer/role histograms + modules + top
  centrality for the whole repo.
- New MCP tool `find_callers` gains an `include_tests` field; the card and
  CLI default to excluding test callers.

### Added

- `<abyss-card>` rendering on pre-edit hook output (six sections:
  where / depends-on / depended-on / contracts / change-coupling / recent).
- `.code-abyss/arch.toml` user override for the layer dictionary; weight +
  layer tag per path-segment rule, plus `ignore.patterns` regex.
- `arch_facts` and `arch_modules` SQL tables (schema v5).
- `files.lang_family` denormalized column (schema v6) backing the
  same-language-family filter.
- Dictionary expanded to 24 default layer rules covering middleware /
  route / service / queue / event / cache / migration / scheduler /
  worker / validator / log / metric / seed / fixture.
- Honest module labels: collisions get `-{module_id}` suffix; mega-
  clusters where the modal directory segment is <50% of members get a
  `mixed:{segment}+` prefix so agents see when a community is cross-
  cutting rather than a clean module.
- New CLI flag: `abyss callers <sym> --include-tests`.
- New CLI flag: `abyss watch --debounce-ms <N>`.
- Hostile-workload bench (`eval/hostile.sh`) measures hook latency p50/p95/
  p99 under burst-edit scenarios (10 / 100 / 500 file batches).

### Changed

- Hook `pre-edit` is read-only — no synchronous reindex. Refresh moves to
  hook `post-edit`. This is what makes ambient delivery viable on real
  repos.
- `build_file_context` collapses its per-symbol N+1 caller lookup into a
  single `symbols ⋈ refs ⋈ files` JOIN. Hub-file latency drops from ~300ms
  to <30ms.
- `find_callers` (and the card render path) excludes test callers by
  default. Add `--include-tests` to see them.
- `abyss where` / `abyss context` file-path matching now prefers exact
  match → prefix match → shortest suffix match. Fixes the "deep nested
  copy beats root file" trap on polyglot repos.
- `abyss search` ranking: file penalty applied after RRF fusion using
  `arch_facts.centrality` × test-path penalty × import-chunk penalty.
  Real implementations now outrank test mocks and import statements.
- Louvain default tuning: γ=1.5 with single-pass (multi-level opt-in via
  `LouvainParams::multi_level`). Module count on the abyss self-index
  went 9 → 12, largest community 18 → 11 files.
- Coupling denominator is now symmetric (min of both files' total
  changes) instead of asymmetric; the global gate suppresses coupling
  entirely below 50 commits ("insufficient history" log line).
- The MCP `search_context` tool stopped advertising semantic search in
  slim builds (which don't ship an embedder); now reports
  `precision_mode: "fulltext"` honestly.

### Fixed

- **Cross-language pollution (dogfood-found)**: the demoted resolver
  tiers (L2 / L3 / L4 / L4b / L5) used to do pure name-match across all
  symbols, so a Rust file calling `e.target()` could link to a JavaScript
  function named `target()`. Same-language-family filter added: rust,
  go, python, java, bash are 1:1 families; ts-family covers
  typescript+tsx+javascript+jsx+mjs+cjs; c-family covers c+cpp+h+hpp.
- BFS visited-set in `impact_analysis` keys on `(file_id, symbol_name)`
  instead of bare name, preventing whole transitive subtrees from being
  silently dropped when two functions in different files share a name.
- `abyss attach claude` uses `dirs::home_dir()` instead of
  `std::env::var("HOME")`, so it works on Windows (`%USERPROFILE%`).

### Eval — gated precision / recall vs SCIP ground truth

| Corpus | Lang | v0.3.6 | v0.4.0 | Δ |
|--------|------|--------|--------|---|
| gin v1.10.0 | Go | 99.3 / 82.6 | **99.3 / 82.6** | 0 / 0 |
| hono v4.6.14 | TypeScript | 98.8 / 63.8 | **98.8 / 63.8** | 0 / 0 |
| click 8.1.8 | Python | 97.9 / 93.0 | **97.9 / 93.0** | 0 / 0 |
| ripgrep 14.1.1 | Rust | 98.5 / 75.3 | **98.5 / 75.3** | 0 / 0 |
| abyss @8099aeb | Rust dogfood | 100.0 / 90.9 | **100.0 / 90.9** | 0 / 0 |
| cmark 0.31.1 | C | 99.1 / 74.8 | **99.1 / 74.8** | 0 / 0 |

All six corpora zero regression. The original baseline for click (98.7 /
94.6 from 2026-06-12) was captured against an older `scip.json` that
contained 573 truth pairs; today's `scip.json` was regenerated by
scip-python v0.6.6 and contains 589 pairs — sixteen additional polymorphic
method dispatches that fall in the resolver's weakest category (base-class
calls). A v0.3.6 binary built today and indexed against the same corpus
produces a byte-equivalent DB and the *same* 97.9 / 93.0 score, so the
delta is a ground-truth refresh, not a code regression. Full diff and
method in [eval/notes/click-microregression-2026-06-17.md](eval/notes/click-microregression-2026-06-17.md).
Baseline updated to 97.9 / 93.0; scip-python version pinned.

### Hostile bench (abyss dogfood, 82 files / ~5000 refs)

| | v0.3.6 (sync hook + N+1) | v0.4.0 | factor |
|---|---|---|---|
| BURST=10 p95 | 171.65 ms | **8.34 ms** | 21× |
| BURST=100 p99 | **1072.43 ms** | **~11 ms** | **96×** |
| BURST=500 p95 | 473.33 ms | **9.34 ms** | 51× |
| card body | 0 bytes (eprintln) | 600–2800 bytes (rendered) | from nothing |

### Schema

- v5 — `arch_facts`, `arch_modules` tables
- v6 — `files.lang_family` denormalized column + `idx_files_lang_family`

Schema upgrades are additive `CREATE TABLE IF NOT EXISTS` / `ALTER TABLE
ADD COLUMN`; existing indexes are forward-compatible.

### Known limitations

- `abyss watch` is a single foreground process (V1 daemon-lite). V2 with
  Unix-socket multi-reader so the hook, MCP server, and editor extension
  share one index is on the roadmap.
- Layer dictionary is regex-only; LLM-based labeling stays out of the hot
  path. Users in non-Western codebases or with non-standard naming should
  drop a `.code-abyss/arch.toml` override (see `docs/ARCH-LAYERS.md`).

---

For pre-v0.4.0 history, see git log.
