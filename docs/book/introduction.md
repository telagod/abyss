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
  `search`. The commands you actually run.
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
