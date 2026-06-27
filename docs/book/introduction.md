# abyss

> **The code graph your agent checks before it edits.**

abyss builds a call graph + temporal intelligence index of your codebase,
so an AI coding agent can answer — in milliseconds, before touching a
file — three questions grep can't:

1. **Who calls this?** — cross-file caller tracing with confidence scores
2. **What breaks if I change it?** — blast-radius analysis with risk
   scoring and test-coverage gaps
3. **Where does this code hurt?** — hotspots (churn × complexity) and
   change-coupled files, mined from git history

No language server. No embedding model required. One static binary, one
SQLite file, second-scale indexing.

```text
$ abyss index
✓ 312 files, 4,209 chunks, 11,487 symbols, 23,961 refs in 4,820ms

$ abyss impact SetError
impact: SetError  direct=17  transitive=521  tests=3  uncovered=319  risk=8.5/10
  ⚠ high blast radius
  ⚠ 319 paths without test coverage
```

abyss is not a search engine replacement — it's the **impact-awareness
layer** agents are missing. Pair it with whatever search you like.

> abyss is currently at **v0.5.27**: **535 tests**, **6 eval corpora**
> all holding ≥98.5% gated precision (Click climbed 97.9% → 99.3%), a
> **90% proxy compression rate**, and an architecture overhaul that split
> `main.rs` into a focused `src/commands/` tree. See the
> [Dogfood chapter](dogfood/index.md) for what running abyss against
> SQLAlchemy / Django / hono / vite / FastAPI / helix-editor actually
> looks like — score, bugs found, falsified predictions, all in public.

## What changed in v0.5.x

The v0.5.x patch sprint (v0.5.3 → v0.5.27, zero breaking changes)
hardened the daemon, matured Python coverage, and sanded a long backlog
of operability rough edges:

- **V2 daemon emerges** — `abyss mcp --via-daemon` (v0.5.2),
  `daemon logs --follow` (v0.5.7), and a `subscribe` socket verb that
  push-notifies reindex events to long-lived agents (v0.5.16).
- **Python coverage matures** — generic-base inherit edges (v0.5.2),
  first-class `type_ref` emission for typed params/returns (v0.5.3),
  `.pyi` stub files indexed end-to-end (v0.5.13), MRO walker pinned
  by regression test on co-located bases (v0.5.17).
- **Dogfood-driven UX** — `callers` default-both type+call (v0.5.0),
  small-cluster module labels by peak centrality (v0.5.2), TSX JSX
  visitor (v0.5.2), Rust / TS built-in filters (v0.5.4 …).
- **Operability** — shell completion (`abyss completion`, v0.5.8),
  `abyss config show` introspection (v0.5.10), `abyss reset` (v0.5.11),
  incremental `abyss index --since <ref>` (v0.5.12), hostile-concurrent
  hook p99 of 82 ms with zero failures (v0.5.5 stress test).

The v0.5.21 → v0.5.27 stretch turned the corner from operability to
audit-driven hardening, a resolver precision breakthrough, and a new
token-compressing proxy:

- **v0.5.27 audit-driven hardening** — daemon socket chmod 0700,
  SQL safety (debug_assert → assert), attach panic fixes. Architecture
  overhaul: main.rs 2489 → 505 lines via `src/commands/` extraction.
- **Resolver precision breakthrough** — same-file priority for
  qualified calls eliminates polymorphic false positives. Click
  97.9% → 99.3% gated precision. All 6 corpora pass 98.5% gate.
- **Token-compressing proxy** — `abyss proxy` intercepts commands
  and compresses output (90% average savings). 28 Rust handlers +
  13 TOML declarative filters. Tree-sitter AST body stripping for
  `cat`, structural parsing for git/cargo/go.
- **Schema v7 performance** — 4 new indices, PRAGMA busy_timeout,
  full reindex 534 → 286ms (−46%).

The single-page [Release Notes](https://github.com/telagod/abyss/blob/main/RELEASE-NOTES.md)
walks through the whole series; per-version detail lives in
[CHANGELOG.md](https://github.com/telagod/abyss/blob/main/CHANGELOG.md).

## Why not just grep / LSP / embeddings?

| | grep | LSP | embedding search | **abyss** |
|---|---|---|---|---|
| Find callers | text matches, noisy | precise, needs running server per language | no | one binary, indexed |
| Blast radius + risk score | no | no | no | yes |
| Hotspot / change coupling (git temporal) | no | no | no | yes |
| Pre-edit agent hook | no | no | no | yes |
| Works offline, zero setup | yes | no | no | yes |

## What this book covers

- **Getting started** — install, your first index, attaching the agent
  hook so the pre-edit card shows up before every edit.
- **Daily use** — `abyss where`, `context`, `impact`, `callers`,
  `search`, plus `proxy` / `gain` for token-compressed command output.
  The commands you actually run.
- **Ambient mode** — `abyss watch` (foreground) and
  `abyss daemon start --detach` (background) keep the index fresh as
  files change.
- **Architecture** — L0 arch coordinates, the tiered resolver, design
  principles. The "why" behind the heuristics.
- **Eval** — how precision and recall are measured against SCIP
  ground truth, per-corpus numbers, and the notes that document
  microregressions and falsifications.
- **Dogfood** — honest field reports from running abyss on helix-editor,
  vite, FastAPI.
