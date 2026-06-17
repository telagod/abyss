# Changelog

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
