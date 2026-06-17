# Attaching the agent hook

abyss exposes itself to AI coding agents in two ways:

## MCP (any MCP-compatible client)

```sh
abyss mcp
```

This starts a stdio MCP server exposing seven tools: `search_context`,
`get_symbols`, `find_callers`, `impact_analysis`, `code_map`,
`evolution`, `index_project`. Point your MCP client at the command;
abyss handles the rest.

## Pre-edit hooks (Claude Code, Codex CLI, Gemini CLI, Pi, Hermes, OpenClaw)

`abyss hook pre-edit` reads the tool-call JSON your agent platform pipes
to its hooks, refreshes the index incrementally, and writes a structured
warning to stderr — **before** the edit happens. Payload shapes are
auto-detected per platform.

The fastest setup is the companion tool:

```sh
# installs per-platform hook configs in one command
# see https://github.com/telagod/code-abyss
```

## The pre-edit card

What the agent actually sees is a compact card formatted by abyss. It
lists the file's architectural layer, top callers, change-coupled
neighbors, hotspot score, and any production-path warnings — distilled
to the minimum the model can act on without re-reading the codebase.

See [The pre-edit card](../daily-use/pre-edit-card.md) for the layout.
