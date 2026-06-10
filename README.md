# abyss

> **The code graph your agent checks before it edits.**

abyss builds a call graph + temporal intelligence index of your codebase, so an AI coding agent can answer — in milliseconds, before touching a file — three questions grep can't:

1. **Who calls this?** — cross-file caller tracing with confidence scores
2. **What breaks if I change it?** — blast-radius analysis with risk scoring and test-coverage gaps
3. **Where does this code hurt?** — hotspots (churn × complexity) and change-coupled files, mined from git history

No language server. No embedding model required. One static binary, one SQLite file, second-scale indexing.

```
$ abyss index
✓ 312 files, 4,209 chunks, 11,487 symbols, 23,961 refs in 4,820ms

$ abyss impact SetError
impact: SetError  direct=17  transitive=521  tests=3  uncovered=319  risk=8.5/10
  ⚠ high blast radius
  ⚠ 319 paths without test coverage
```

## Why not just grep / LSP / embeddings?

| | grep | LSP | embedding search | **abyss** |
|---|---|---|---|---|
| Find callers | text matches, noisy | ✅ precise, needs running server per language | ❌ | ✅ one binary, indexed |
| Blast radius + risk score | ❌ | ❌ | ❌ | ✅ |
| Hotspot / change coupling (git temporal) | ❌ | ❌ | ❌ | ✅ |
| Pre-edit agent hook | ❌ | ❌ | ❌ | ✅ |
| Works offline, zero setup | ✅ | ❌ | ❌ | ✅ |

abyss is not a search engine replacement — it's the **impact-awareness layer** agents are missing. Pair it with whatever search you like.

## Install

```sh
# from source (Rust toolchain required)
./install.sh

# prebuilt binaries: coming in v0.3.0 (GitHub Releases / npx @code-abyss/cli)
```

## Quickstart (60 seconds)

```sh
cd your-project
abyss index                 # build structural index (~seconds)
abyss map                   # hotspots + coupling overview
abyss context src/auth.go   # full context before editing a file
abyss impact ValidateToken  # blast radius of changing a symbol
```

Every command takes `--json` for machine consumption.

## Commands

```
abyss index                   Structural index: symbols, refs, fulltext, git temporal. Seconds.
abyss context <file>          Everything an agent needs before editing: callers, deps, risk, coupling
abyss callers <symbol>        Who calls this (with confidence %)
abyss impact <symbol>         Blast radius: direct/transitive callers, uncovered paths, risk 0-10
abyss history <file>          Evolution: commits, churn, coupled files [--symbol <fn>]
abyss search "query"          Symbol + fulltext fusion search
abyss map                     Codebase map: hotspots, coupling, risk areas
abyss stats                   Index statistics
abyss mcp                     MCP server (stdio) — 7 tools for any MCP client
```

## Language support

Caller tracing & impact analysis (reference extraction):

| Language | Calls | Type refs | Imports |
|----------|-------|-----------|---------|
| Go | ✅ | ✅ | ✅ |
| Rust | ✅ | ✅ | ✅ |
| TypeScript / TSX | ✅ | ✅ | ✅ |
| JavaScript | ✅ | ✅ | ✅ |
| Python | ✅ | ✅ | ✅ |

Symbol indexing & search additionally cover Java, C, C++, JSON, TOML, YAML, Bash, HTML, CSS. Java/C/C++ caller tracing is on the roadmap.

## How resolution works (and how honest it is)

References resolve through tiered heuristics, each tagged with a confidence score stored in the index:

| Tier | Strategy | Confidence |
|------|----------|-----------|
| 1 | Same file | 1.0 |
| 2 | Same package/directory | 0.95 |
| 3 | Import-qualifier match | 0.9 |
| 4 | Globally unique symbol | 0.8 |
| 5 | Ambiguous (first candidate) | 0.5 |

This is not a compiler — precision/recall benchmarks against SCIP ground truth are planned for v0.4 and will be published here, whatever the numbers say.

## Agent integration

**MCP**: `abyss mcp` exposes `search_context`, `get_symbols`, `find_callers`, `impact_analysis`, `code_map`, `evolution`, `index_project` over stdio.

**Pre-edit hooks**: [code-abyss](https://github.com/telagod/code-abyss) installs hooks for Claude Code, Codex CLI, Gemini CLI, Pi, Hermes, and OpenClaw that automatically run `abyss context` before any code edit — the agent sees production callers and hotspot warnings without being asked.

## Features

| Build | Contents | Size |
|-------|----------|------|
| default (slim) | call graph + temporal + fulltext + MCP | ~18M |
| `--features semantic` | + embedding-based semantic search (fastembed/ONNX) | ~43M |

## Status

v0.3.0-dev — APIs and index format may change. See [docs/DESIGN-v0.3.md](docs/DESIGN-v0.3.md) for the roadmap: test suite + SCIP-based eval harness, prebuilt binaries, single-binary hooks, agent A/B regression benchmark.

## License

Apache-2.0
