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

Host integration is split by hook-surface shape — a stable architectural
contract as of v0.5.24:

| Host           | Owner                | Reason                                                      |
|----------------|----------------------|-------------------------------------------------------------|
| Claude Code    | `abyss attach`       | Shared `~/.claude/settings.json`, idempotent JSON edit      |
| Codex CLI      | `abyss attach`       | Shared `~/.codex/config.toml` (Codex 0.125+ array tables)   |
| Gemini CLI     | `abyss attach`       | Shared `~/.gemini/settings.json`                            |
| OpenClaw       | `code-abyss` (npm)   | Per-pack layout `packs/abyss/openclaw/`, not a settings file |
| Pi             | `code-abyss` (npm)   | Hook-config shape still evolving across versions            |
| Hermes         | `code-abyss` (npm)   | Hook-config shape still evolving across versions            |

### Production install: `abyss attach` (claude / codex / gemini)

For the three hosts whose hook config lives in a single shared settings
file, `abyss attach` is the production main entrypoint:

```sh
abyss attach claude     # ~/.claude/settings.json
abyss attach codex      # ~/.codex/config.toml (Codex 0.125+ array tables)
abyss attach gemini     # ~/.gemini/settings.json (SessionStart/BeforeTool/AfterTool)
abyss attach all        # all three; openclaw surfaced as a skipped row
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

All three installers are idempotent: re-running upgrades in place,
never duplicates entries, and preserves any unrelated keys you (or
another tool) put in the settings file.

### Architectural delegation: `code-abyss` (openclaw / pi / hermes)

OpenClaw, Pi, and Hermes have hook surfaces the abyss binary cannot
reliably manage from a single static install:

* **OpenClaw** uses a **per-pack install layout** (`packs/abyss/openclaw/`)
  rather than a shared settings file. A single binary cannot reliably
  create that per-pack directory tree across user workspaces. Running
  `abyss attach openclaw` directly exits with a migration message;
  `abyss attach all` surfaces it as a `skipped: …` row.
* **Pi & Hermes** hook-config shapes are still evolving across versions.
  Shipping a best-effort installer from abyss would risk breaking user
  configs on every host version bump; the npm package's adapters can
  iterate independently of abyss's release cadence.

These three hosts are owned by the companion
[`code-abyss`](https://github.com/telagod/code-abyss) npm package:

```sh
npx code-abyss -t openclaw --with-hooks
npx code-abyss -t pi       --with-hooks
npx code-abyss -t hermes   --with-hooks
```

> code-abyss v4.8.x still accepts the legacy `--with-abyss` flag; that
> flag enters deprecation in v4.9.0 and is removed in v5.0. For
> openclaw/pi/hermes, switch to `--with-hooks` — the forward-compatible
> name that survives the v5.0 cut.

### Migrating from `code-abyss --with-abyss` (claude / codex / gemini)

If you previously ran `npx code-abyss -t claude --with-abyss` (or
`codex` / `gemini`), that flag is being **deprecated in code-abyss
v4.9.0** and **removed in v5.0**. The shared-settings-file hosts move
to `abyss attach` as the single source of truth:

```sh
# old (code-abyss v4.8.x, deprecated)
npx code-abyss -t claude --with-abyss

# new (v4.9+; the installer is idempotent and replaces any prior shape)
abyss attach claude
```

The installer is idempotent — re-running it overwrites the previous
shape in place, so the migration is just "run the new command once,
done." For openclaw/pi/hermes the recommendation is unchanged: keep
using `code-abyss`, just rename the flag to `--with-hooks`.

## The pre-edit card

What the agent actually sees is a compact card formatted by abyss. It
lists the file's architectural layer, top callers, change-coupled
neighbors, hotspot score, and any production-path warnings — distilled
to the minimum the model can act on without re-reading the codebase.

See [The pre-edit card](../daily-use/pre-edit-card.md) for the layout.
