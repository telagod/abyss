# Changelog

## v0.5.25 — 2026-06-26

The "windows build fix" patch — fixes a chronic build regression that
silently affected v0.5.14 → v0.5.24 (5+ release cycles).

### Fixed

- **Windows build no longer fails with `unresolved import regex/petgraph`.**
  `Cargo.toml` had `petgraph` and `regex` declared *after* a
  `[target.'cfg(unix)'.dependencies]` table opened (originally just for
  `libc`), which TOML parsed as making those two crates **unix-only**.
  Result: every `cargo build` / `cargo test` on Windows since v0.5.14
  failed to compile (11 `E0432` / `E0433` errors). `arch::naming`,
  `arch::dictionary`, `arch::override_config` need `regex`;
  `arch::graph`, `arch::inference` need `petgraph` — all five modules
  are platform-agnostic core code, not unix-only.
  Fix: moved `petgraph` and `regex` to the top-level `[dependencies]`
  section. Relocated the `[target.'cfg(unix)'.dependencies]` table to
  the end with a regression-guard comment so future deps added after
  it don't fall into the same trap.

### Why this slipped 5 releases

Linux + macOS CI both pass; only Windows fails, and Windows had been
red continuously since v0.5.14 (see release.yml history). The pattern
was filed under "Windows CI is chronically broken" rather than
investigated. A docs-only PR (v0.5.24) running through the same
matrix surfaced the same failure and made the root cause obvious.

### Not changed

- All v0.5.24 docs / comments / error messages (still in effect).
- Hook shapes, skill-manifest schema, hook payload — all identical.
- `cargo test --lib attach` 26/26 passes on Linux (no regression).

## v0.5.24 — 2026-06-26

The "positioning reversal" docs+notes patch. As of v0.5.23 the docs
called `abyss attach` a "cargo-only fallback" and pointed users at
`npx code-abyss -t <host> --with-abyss` as the production entrypoint.
With the sister `code-abyss` package landing its v4.9.0 deprecation /
v5.0 cut of host-side hook integration (claude/codex/gemini moving to
abyss attach; openclaw/pi/hermes staying with code-abyss), the
positioning reverses: **`abyss attach` is the production main
entrypoint** for the three shared-settings-file hosts; the companion
package keeps the per-pack and unstable-shape hosts.

This release is documentation + comment changes only — no behavior
changes, no new crates published, no schema bumps.

### Changed

- `src/attach/mod.rs` — module docs upgraded from "supported hosts
  today" framing to an explicit 6-host responsibility split, stable as
  of v0.5.24. openclaw/pi/hermes are explicitly delegated to the
  companion `code-abyss` package, not "not yet wired."
- `src/attach/openclaw.rs` — module docs reframe the openclaw no-op
  from "intentional downgrade" to "architectural delegation." The
  `bail_with_message()` migration string now points at
  `npx code-abyss -t openclaw --with-hooks` (the forward-compatible
  flag), with a note that `--with-abyss` is the v4.8.x legacy name
  entering deprecation in code-abyss v4.9.0.
- `README.md` — `Pre-edit hooks` section restructured into an explicit
  "abyss attach (production)" vs "code-abyss (architectural
  delegation)" split. The Commands table's `abyss attach <host>` row
  marks openclaw as `openclaw*` with a footnote pointing at the
  delegation.
- `docs/book/getting-started/agent-hook.md` — full rewrite of the
  "Pre-edit hooks" page: 6-host responsibility table at the top;
  `abyss attach` as the recommended production install; a new
  "Migrating from `code-abyss --with-abyss`" subsection guides users
  off the deprecated flag onto `abyss attach`.

### Why now

`code-abyss` shipped a v4.9.0 deprecation entry today: `--with-abyss` /
`--with-mcp` go to warning, and `--with-hooks` narrows to
openclaw/pi/hermes only. Removing them in v5.0 requires that abyss own
its half of the integration plainly in docs before users hit the
deprecation warnings; otherwise the migration message points at a path
the docs still call "cargo-only fallback."

### Not changed

- Hook shapes (claude/codex/gemini) — identical to v0.5.23.
- skill-manifest schema — `schema_version=1`, identical to v0.5.22.
- Hook payload format — identical to v0.5.x.
- All 386 existing tests pass without modification; openclaw's
  `install_at_returns_clear_error` test still passes (the new error
  message still contains `npx code-abyss`).

## v0.5.23 — 2026-06-18

The "ground-truth attach shapes" fix. v0.5.21 shipped `abyss attach
codex/gemini/openclaw` with best-effort schemas; a cross-check against
the production-tested adapters in the sister `code-abyss` package
revealed all three were wrong. v0.5.23 corrects each against the
sister project's shape contract.

### Fixed

- **`abyss attach codex`** — now emits Codex 0.125+ **two-level
  array-of-tables** (`[[hooks.Event]]` + `[[hooks.Event.hooks]]`).
  The old flat `[hooks.X]` map was REJECTED by Codex with
  `invalid type: map, expected a sequence in hooks`. New events:
  `SessionStart` (matcher `startup|resume`, timeout 10s),
  `PreToolUse` and `PostToolUse` (matcher `Bash|shell`, timeout 5s).
- **`abyss attach gemini`** — now uses Gemini-native event names
  (`SessionStart`, `BeforeTool`, `AfterTool`) with Gemini-native
  matchers (`startup`, `write_file|replace|edit_file`). Hook entries
  are `{name, type, command, timeout, description}` objects with
  timeout in **milliseconds** (Codex uses seconds — don't mix).
  v0.5.21 incorrectly copied Claude's `PreToolUse/PostToolUse +
  Edit|Write` shape; those are Claude-only.
- **`abyss attach openclaw`** — intentionally **downgraded to a clear
  error**. OpenClaw uses a per-pack install layout
  (`packs/abyss/openclaw/`), not a settings file. Shipping a
  `~/.openclaw/config.toml` stanza that OpenClaw never reads is worse
  than no-op — it silently fails. Users are pointed at
  `npx code-abyss -t openclaw --with-abyss` for the working install.
- **`abyss attach all`** — surfaces `openclaw` as a `skipped: …` row
  rather than tagging it installed or failing the batch.

### Changed

- `src/manifest.rs` `providers.hooks.attach_notes.openclaw` annotates
  the downgrade so machine-readable consumers know the command is a
  no-op rather than silently misleading them.
- Docs (`docs/book/getting-started/agent-hook.md`) call out that
  `abyss attach` is the cargo-only fallback; production multi-host
  installs with the latest schemas should use
  `npx code-abyss -t <host> --with-abyss`.

### Ground truth

Shape decisions are anchored against the sister `code-abyss` package
(local at `/home/telagod/project/code-abyss/bin/{lib/abyss-integration.js,
adapters/codex.js, adapters/openclaw.js}`), which has been
production-tested across Claude Code, Codex 0.125+, Gemini CLI, and
OpenClaw.

## v0.5.22 — 2026-06-18

The "interop with code-abyss" patch. abyss now emits a single
machine-readable manifest that sister tools (the companion `code-abyss`
package and any other skill-discovery consumer) can read instead of
hand-coding the integration.

### Added

- **`abyss skill-manifest`** — emits a JSON document describing the CLI
  surface (every subcommand + one-line summary), the MCP tool list
  (8 tools), the hook entry points, the four `attach` hosts, and the
  daemon socket verbs. Defaults to pretty-printed; `--compact` collapses
  to a single line for machine pipelines.
- **`schema_version: 1`** — a single integer at the top level so
  consumers can pin a known-good shape. Bumped only on breaking shape
  changes; adding a new field or new array element is free.
- **`src/manifest.rs`** — pure builder that returns a `serde_json::Value`.
  Unit tests assert the manifest structure without spawning the binary.

### Design

- The manifest is the contract. Adding a CLI command, MCP tool, or
  attach host requires updating `src/manifest.rs` in the same patch.
- Sister tools should not have to ask agents to integrate — the
  manifest answers what's exposed before they have to ask.
- Documented as principle §11 in `docs/PRINCIPLES.md` ("co-existence
  with sister tools via machine-readable manifests").

## v0.5.21 — 2026-06-18

The "agent ecosystem catch-up" patch. `abyss attach` grows from
Claude-only to the full code-abyss host roster.

### Added

- **`abyss attach codex`** — installs hooks into `~/.codex/config.toml`
  (or `<cwd>/.codex/config.toml` with `--local`). TOML
  `[[hooks.PreToolUse]]` / `[[hooks.PostToolUse]]` blocks with the
  same idempotency contract as the Claude installer.
- **`abyss attach gemini`** — installs hooks into
  `~/.gemini/settings.json`. JSON shape mirrors Claude's so existing
  hook-config muscle memory carries over.
- **`abyss attach openclaw`** — installs hooks into
  `~/.openclaw/config.toml`.
- **`abyss attach all`** — fan-out installer that runs every host's
  installer and prints a per-host summary (`installed` /
  `already present` / `skipped: ~/.host does not exist`). In `--local`
  mode no host is skipped — useful for end-to-end testing.

### Notes

- Codex / Gemini / OpenClaw hook schemas are still maturing. The
  installers ship a best-effort layout and log a "please file an issue
  if this doesn't match your version" line. Re-runs never duplicate
  entries, and existing unrelated config keys are preserved verbatim.
- Pi and Hermes are not wired here yet — their hook shapes are not
  stable enough across versions for an opinionated installer. Use the
  companion `code-abyss` package for those until the shapes settle.
- `attach::install_at` is now `pub` on every host module so external
  tests (and the companion `code-abyss` package) can drive a tempdir
  target without mutating process-wide `$HOME` / cwd.

## v0.5.2 — 2026-06-17

The "more dogfood than features" release. Six small lines of work, no
breaking changes — opt-in features and quiet bug fixes only.

1. **V2 daemon: MCP-over-socket**. `abyss mcp --via-daemon` tunnels the
   agent's stdio through a running daemon. Backward compatible —
   standalone `abyss mcp` unchanged; the pre-edit hook still reads SQLite
   directly to keep its sub-12ms budget. Per-connection read-only WAL
   handles let multiple MCP clients share the same daemon.

2. **impact aligns with callers** — both now return the full superset
   of dependent kinds (call + field_access + type_ref + inherit) by
   default. Add `--calls-only` for the legacy function-only blast radius.
   Closes the most-asked v0.5.1 confusion: "why is impact direct=2 but
   callers prod=20?"

3. **Python generic-base inheritance** — `class Sub(Base[T]):` used to
   silently drop the `Base` inherit ref via a `_ => continue` arm.
   SQLAlchemy's `callers ColumnElement` jumps from 13 to 35; FastAPI's
   `class Sub(BaseModel)` chains stay correct; typing-system markers
   (Generic, Protocol, TypeVar, Callable, Union, …) are denylisted.

4. **TSX visitor extracts JSX refs** — component-name calls
   (`<Foo …/>` → kind=call, Pascal-gated to skip HTML intrinsics) and
   attribute-binding expressions (`{someValue}` in JSX) now feed the
   resolver. Drops the `.tsx` un-resolved-ref rate observed on hono.

5. **Small-cluster module labels by peak centrality** — when a Louvain
   community has <8 members, the labeller switches from weighted-sum to
   peak-centrality tiebreaker. Fixes FastAPI's `param_functions.py`
   small cluster mis-labelling as `docs_src`.

6. **Honest UX**: `abyss callers --all-deps` is an explicit alias for the
   default (no-flag) behaviour, and the `--help` text spells each kind
   filter's exact ref-kind set so users don't guess.

### SQLAlchemy MRO validation

The v0.4.0 L0e MRO tier got a third data point. Predicted ≥500 hits on
SQLAlchemy's declarative-base hierarchies (from the Django note); actual
**4,496 hits** — 9× the prediction floor. L0e/file ratio: SQLAlchemy
**6.55** > Django 3.39 (dense mixin towers vs wide tree).

| Project | L0e hits | L0e/Python file | Hierarchy shape |
|---------|---------:|----------------:|-----------------|
| FastAPI | 0 | 0.00 | External roots (Starlette, pydantic) |
| click | 2 | 0.02 | Co-located in core.py |
| Django | 9,450 | 3.39 | Wide tree (Model + Forms + Admin + CBVs) |
| SQLAlchemy | 4,496 | **6.55** | Dense mixin tower (ColumnElement, TypeEngine) |

See `docs/dogfood/sqlalchemy-2026-06-17.md` and
`eval/notes/sqlalchemy-mro-validation-2026-06-17.md`.

### Test count

348 tests pass (was 336 at v0.5.1, +12).

### Eval (SCIP corpora, unchanged)

All six corpora identical to v0.5.0/v0.5.1 — none of this work changed
resolver scoring tiers. The Python generic-base fix may produce a small
positive Δ on the click corpus next round (when the eval is re-run with
SQLAlchemy-style fixtures in the truth set).

---

## Unreleased — V2 daemon

### Added

- **MCP-over-socket** — the daemon now serves the standard 7-tool MCP
  surface on its Unix socket via a new `{"cmd":"mcp"}` verb. After
  the switch line the connection becomes a JSON-RPC channel served
  by an embedded `rmcp` instance. Each MCP-mode connection gets its
  own [`Repository::open_read_only`](src/storage/repo.rs) handle so
  multiple MCP clients sharing one daemon don't contend with the
  watcher's writer (WAL: N readers + 1 writer). See
  [docs/book/ambient/socket-protocol.md](docs/book/ambient/socket-protocol.md).
- **`abyss mcp --via-daemon`** — agent-facing client. Connects to
  `<workspace>/.code-abyss/daemon.sock`, sends the `mcp` verb, and
  pipes the agent's stdio bidirectionally through the socket. Errors
  out (does not silently fall back) when no daemon is running:
  ```
  abyss mcp --via-daemon: no daemon found at <socket>;
  either start one with `abyss daemon start` or drop --via-daemon
  for standalone mode
  ```

### Design notes

- The pre-edit hook deliberately stays on the direct-SQLite path. A
  socket hop would regress its sub-12ms budget for no agent-visible
  benefit — only `abyss mcp --via-daemon` uses the new MCP-over-socket
  path.
- `abyss mcp` (no flag) is unchanged. Existing MCP configs migrate by
  appending `--via-daemon`, not by replacing the standalone binary.
- WAL `journal_mode` is set once by the writer in
  `storage::schema::init_db`; read-only handles use the new
  `init_db_read_only` helper to skip the `journal_mode` PRAGMA (which
  would fail on a read-only connection).

## v0.5.1 — 2026-06-17

v0.5.1 closes the dogfood-surfaced debts from Django + hono. The
Django run (8/10) surfaced two precision bugs that the hono run
(8/10) had already flagged from a different angle — `callers` was
silently filtering kinds it had data for, and the L0e sibling
collision pattern showed up the moment a real in-repo MRO corpus
landed. This release fixes both, plus the dictionary expansion and
the `--limit` knob the hono report explicitly asked for.

### Fixed

- **`abyss callers` now surfaces `kind='inherit'` edges by default**
  (Django B1, [dogfood/django-2026-06-17.md](docs/dogfood/django-2026-06-17.md)).
  Pre-v0.5.1, `callers Model` on the Django corpus returned 7 hits
  (random test classes named `Model`) while the DB held 983
  `inherit` refs at confidence ≥ 0.9 pointing at the real
  `django.db.models.Model`. Same shape as the v0.5.0 `kind='call'`
  bug that vite surfaced for `ViteDevServer`. Default kinds are
  now `{call, type_ref, inherit}`; the `(type, 95%)` / `(inherit,
  95%)` annotations make the edge kind first-class in the output.
- **L0e sibling-name disambiguation** (Django B2,
  [dogfood/django-2026-06-17.md](docs/dogfood/django-2026-06-17.md)).
  `DatabaseSchemaEditor` is defined in four backends
  (`oracle/`, `mysql/`, `postgresql/`, `sqlite3/`); pre-v0.5.1
  every `self.execute()` from any backend resolved into
  `postgresql/schema.py` because the MRO walker picked
  first-globally. L0e now prefers the receiver-type definition
  that shares the source file's directory before falling back to
  globally-unique. Same fix pattern as L2's same-pkg-unique guard.
- **`callers` display cap is no longer silent** (hono B1,
  [dogfood/hono-2026-06-17.md](docs/dogfood/hono-2026-06-17.md)).
  `callers Context` used to truncate to 20 rows with no footer; on
  hono the DB has 235 high-confidence refs and 215 were hidden.
  Output now includes a `(showing N of M)` footer when the result
  set exceeds the limit.
- **`callers` filters JS/TS built-in name shadows** (hono B3,
  [dogfood/hono-2026-06-17.md](docs/dogfood/hono-2026-06-17.md)).
  Hono's `Set (interface)` collided with the JS global `Set` — 17
  reported callers were mostly the global, not the Hono symbol.
  L4 / L5 now bias toward leaving unresolved when the symbol name
  matches a JS/TS built-in lexicon (`Set`, `Map`, `Promise`,
  `Error`, `Date`, `RegExp`, `Array`, `Object`).

### Added

- **`abyss callers <sym> --inherits-only`** — restrict results to
  `kind='inherit'`. The "who subclasses this?" question is now a
  first-class flag, complementing `--calls-only` and `--types-only`.
- **`abyss callers <sym> --limit N`** — explicit cap on rows
  printed. Default stays at 20; `--limit 0` means no cap.
- **Dictionary expansion for framework vocabulary** (hono debt,
  v0.5.0 carry-over). The layer dictionary now recognises
  framework-entry filenames (`hono.ts`, `vite.ts`, `app.ts`,
  `application.py`, `manage.py`, `wsgi.py`, `asgi.py`,
  `setup.py`, `urls.py`, `views.py`, `models.py`, `forms.py`,
  `admin.py`, `settings.py`) so `where src/hono.ts` no longer
  returns `layer=unknown conf=0.00`.

### Known limitations / V2 items

These debts surfaced by the hono dogfood are tracked but not in
v0.5.1:

- **`impact direct=N` vs `callers prod=N` semantic gap.** On hono,
  `impact Context` reports `direct=2` while `callers Context`
  reports `prod=20` — same DB, same confidence threshold. The two
  commands apply different definitions of "direct caller" (impact
  filters to refs targeting the canonical-file declaration of the
  symbol; callers includes refs to any same-named symbol). The
  gap is documented but the two commands don't yet share a caller
  selector. Unification is a V2 item — both need a shared
  `caller_set(sym, opts)` primitive.
- **Centrality-weighted module labelling still drowns hub files in
  tiny clusters.** `where django/db/models/base.py` returns
  `module=tests-27` because 1 800+ test files inherit from
  `Model` and bidirectional clustering drags the hub into the
  satellite cluster. Same root cause as FastAPI's `cluster-2`
  swallowing `routing.py`. Fix path is to weight `inherit` /
  `type_ref` edges differently from `call` in the Louvain pass,
  or prefer the source-file's directory prefix as a tiebreaker.
  Pending.
- **TSX un-resolution rate (66.8%) higher than `.ts` (58.9%)** on
  hono. Separate parser issue — JSX intrinsic elements and
  component refs need their own L0-tier binding pass. Tracked
  under TS extractor follow-up.
- **`--inherits-only` filter still uses `kind='inherit'` alone.**
  We need a "all type-position uses" alias that means
  `kind IN ('call', 'type_ref', 'inherit', <future kinds>)` so
  that future kind additions (e.g. `field_access`,
  `decorator_use`) don't silently disappear from `callers`
  again. Filed; the alias is a v0.5.2 candidate.

### Eval — gated precision / recall vs SCIP ground truth (v0.5.1)

No resolver-tier behavior changes affect the SCIP-ground-truth
scoring axis in v0.5.1 (the L0e disambiguation only swaps
sibling-target picks; the call/file pair stays compiler-equivalent
for click and the Django dogfood is not part of the SCIP eval set).
All six SCIP corpora hold at v0.5.0 numbers:

| Corpus | Lang | v0.5.0 | v0.5.1 | Δ |
|--------|------|--------|--------|---|
| gin v1.10.0 | Go | 99.3 / 82.6 | **99.3 / 82.6** | 0 / 0 |
| hono v4.6.14 | TypeScript | 98.8 / 63.8 | **98.8 / 63.8** | 0 / 0 |
| click 8.1.8 | Python | 97.9 / 93.0 | **97.9 / 93.0** | 0 / 0 |
| ripgrep 14.1.1 | Rust | 98.5 / 75.3 | **98.5 / 75.3** | 0 / 0 |
| abyss dogfood | Rust | 100.0 / 90.9 | **100.0 / 90.9** | 0 / 0 |
| cmark 0.31.1 | C | 99.1 / 74.8 | **99.1 / 74.8** | 0 / 0 |

### Schema

No schema bumps in v0.5.1. v6 (added in v0.4.0) is the current
version.

---

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

3. **Four honest dogfood reports**. We ran abyss on hono (8/10),
   helix-editor (7.5/10), vite (7/10), FastAPI (6.5/10) and committed
   the reports as `docs/dogfood/*.md`. They surface real debts and one
   falsified hypothesis: MRO L0e doesn't fire on FastAPI because its
   class hierarchy roots are in external libraries (Starlette, pydantic).
   We log the prediction error so future MRO discussion stays honest.
   The hono report was backfilled post-v0.5.0 (the original v0.3.x walk
   drove the W1 UX debts that v0.4.0 fixed) so the deltas v0.5.0 G1/G2/G3
   closed are measured against the same workload.

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
- 4 dogfood reports: hono (8/10), helix-editor (7.5/10), vite (7/10),
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
