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

## Pre-edit hooks (Claude Code, Codex CLI, Gemini CLI, OpenClaw, Pi, Hermes)

`abyss hook pre-edit` reads the tool-call JSON your agent platform pipes
to its hooks, refreshes the index incrementally, and writes a structured
warning to stderr — **before** the edit happens. Payload shapes are
auto-detected per platform.

### Production install (recommended)

For a full multi-host install with the **latest per-host schemas**
(Codex 0.125+ array-of-tables, Gemini `SessionStart`/`BeforeTool`,
OpenClaw per-pack layout), use the companion `code-abyss` npm package:

```sh
npx code-abyss -t claude   --with-abyss
npx code-abyss -t codex    --with-abyss
npx code-abyss -t gemini   --with-abyss
npx code-abyss -t openclaw --with-abyss
```

`code-abyss` is the single source of truth for shape adapters and is
updated alongside upstream agent releases.

### Cargo-only fallback: `abyss attach`

If you only have the cargo-installed binary on hand, `abyss attach` ships
adapters for three of the four hosts:

```sh
abyss attach claude     # ~/.claude/settings.json
abyss attach codex      # ~/.codex/config.toml (Codex 0.125+ array tables)
abyss attach gemini     # ~/.gemini/settings.json (SessionStart/BeforeTool/AfterTool)
abyss attach all        # all of the above; openclaw skipped (see note)
abyss attach claude --local   # write into <cwd>/.<host>/ instead of $HOME
```

What gets written:

* **Claude** — `SessionStart` + `PreToolUse` + `PostToolUse` with
  matcher `Edit|Write` and timeouts in seconds.
* **Codex** — **two-level array-of-tables** (Codex 0.125+ requires
  this; the old flat `[hooks.X]` map is rejected). `SessionStart`
  (matcher `startup|resume`, timeout 10s), `PreToolUse` and
  `PostToolUse` (matcher `Bash|shell`, timeout 5s).
* **Gemini** — Gemini-native events: `SessionStart` (matcher `startup`),
  `BeforeTool` (matcher `write_file|replace|edit_file`), `AfterTool`
  (same matcher). Each hook entry carries `{name, type, command,
  timeout, description}` with timeout in **milliseconds**.
* **OpenClaw** — **not supported by `abyss attach`** in v0.5.23+.
  OpenClaw uses a per-pack install layout (`packs/abyss/openclaw/`),
  not a shared settings file. Use `npx code-abyss -t openclaw
  --with-abyss` instead. Running `abyss attach openclaw` directly will
  exit with a clear migration message; `abyss attach all` skips the
  host with the same note.

All installers (claude, codex, gemini) are idempotent: re-running
upgrades in place, never duplicates entries, and preserves any unrelated
keys you (or another tool) put in the settings file.

For Pi and Hermes — whose hook shapes are still evolving — use the
companion [`code-abyss`](https://github.com/telagod/code-abyss)
package, which carries shape adapters per host.

## The pre-edit card

What the agent actually sees is a compact card formatted by abyss. It
lists the file's architectural layer, top callers, change-coupled
neighbors, hotspot score, and any production-path warnings — distilled
to the minimum the model can act on without re-reading the codebase.

See [The pre-edit card](../daily-use/pre-edit-card.md) for the layout.
