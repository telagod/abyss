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
# prebuilt binary (linux/macos, x64/arm64) with source-build fallback
curl -fsSL https://raw.githubusercontent.com/telagod/abyss/main/install.sh | bash

# mirror, for networks where raw.githubusercontent.com is unreachable
curl -fsSL https://cdn.jsdelivr.net/gh/telagod/abyss@main/install.sh | bash

# via npm (wrapper downloads the prebuilt binary on install)
npm install -g @code-abyss/cli

# via cargo (binstall fetches the prebuilt binary; install builds from source)
cargo binstall code-abyss   # or: cargo install code-abyss

# or force a source build from a checkout
./install.sh --from-source
```

Windows: `npm install -g @code-abyss/cli`, `cargo binstall code-abyss`, or prebuilt `.zip` on [GitHub Releases](https://github.com/telagod/abyss/releases).

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
abyss callers <symbol>        Who calls this (confidence %, --min-confidence 0 reveals guesses)
abyss impact <symbol>         Blast radius: direct/transitive callers, uncovered paths, risk 0-10
abyss hook pre-edit           Agent hook: tool-call JSON on stdin → refresh index → warn on stderr
abyss hook post-edit          Agent hook: incremental refresh after an edit
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
| Java | ✅ | ✅ | ✅ |

Symbol indexing & search additionally cover C, C++, JSON, TOML, YAML, Bash, HTML, CSS. C/C++ caller tracing is on the roadmap.

## How resolution works (and how honest it is)

References resolve through tiered heuristics, each tagged with a confidence score stored in the index:

| Tier | Strategy | Confidence |
|------|----------|-----------|
| 0 | Receiver-type match (`x.M()` where `x: T` is statically inferrable) | 0.95 |
| 0b | Named-import binding (`import { x } from './m'`, `from m import x`, `import com.f.X`; barrel re-export chains chased) | 0.95 |
| 1 | Same file (bare + self-like calls only) | 1.0 |
| 2 | Same package/directory, unique candidate | 0.95 |
| 3 | Import-qualifier match, unique candidate | 0.9 |
| 4 | Globally unique symbol (member-shaped for qualified calls) | 0.8 |
| 5 | Same package, multiple candidates (demoted) | 0.6 |
| 6 | Same-file fallback for qualified calls / ambiguous | 0.6 / 0.5 |

Receiver types are inferred lite — method receivers, typed parameters,
`x := T{}` / `new T()` / `NewT()` / `x = Type()` declarations, `this`/`self` —
no data-flow, no interface resolution. When a receiver's type is known, only
type-consistent evidence may resolve the call; name-only proximity guesses
demote instead. Full confidence is reserved for call shapes measured ≥98%
correct (bare and self-like calls); qualified calls with an unknown receiver
never pose as facts.

This is not a compiler. Measured against SCIP (compiler-grade) ground truth — published whatever the numbers say:

| Corpus | Language | Gated precision | Gated recall |
|--------|----------|----------------:|-------------:|
| gin v1.10.0 | Go | **99.2%** | **82.6%** |
| hono v4.6.14 | TypeScript | **98.5%** | 58.5%* |
| click 8.1.8 | Python | **98.7%** | **94.2%** |

\* hono assigns router verbs (`app.get/post/use`) at runtime — statically
unresolvable by design; they surface as `possible_callers`.

abyss indexed gin in **~150ms**; scip-go took ~40s. Full method, per-tier tables, and known weaknesses: [eval/RESULTS.md](eval/RESULTS.md). Reproduce: `eval/run.sh`.

## Agent integration

**MCP**: `abyss mcp` exposes `search_context`, `get_symbols`, `find_callers`, `impact_analysis`, `code_map`, `evolution`, `index_project` over stdio.

**Pre-edit hooks**: `abyss hook pre-edit` reads the tool-call JSON any agent platform pipes to its hooks (Claude Code, Codex CLI, Gemini CLI, Pi, Hermes, OpenClaw payload shapes auto-detected), refreshes the index incrementally, and warns about production callers, ambiguous references, and hotspots — before the edit happens. [code-abyss](https://github.com/telagod/code-abyss) installs the per-platform hook configs in one command.

## Features

| Build | Contents | Size |
|-------|----------|------|
| default (slim) | call graph + temporal + fulltext + MCP | ~18M |
| `--features semantic` | + embedding-based semantic search (fastembed/ONNX) | ~43M |

## Status

**v0.3.0** — 45 tests, prebuilt binaries for 5 platforms, single-binary agent hooks. APIs and index format may still change before 1.0. See [docs/DESIGN-v0.3.md](docs/DESIGN-v0.3.md) for the roadmap: SCIP-based eval harness (precision/recall, published whatever the numbers say), Java support, agent A/B regression benchmark.

## License

Apache-2.0
