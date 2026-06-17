# Socket protocol

The V2 daemon socket speaks newline-delimited JSON over a Unix
socket at `.code-abyss/daemon.sock`. Five verbs.

```sh
echo '{"cmd":"ping"}'     | nc -U .code-abyss/daemon.sock
echo '{"cmd":"stats"}'    | nc -U .code-abyss/daemon.sock
echo '{"cmd":"reindex"}'  | nc -U .code-abyss/daemon.sock
echo '{"cmd":"logs","tail":50}' | nc -U .code-abyss/daemon.sock
echo '{"cmd":"mcp"}'      | nc -U .code-abyss/daemon.sock  # then JSON-RPC
```

## Verbs

### `{"cmd":"ping"}`

Returns `{"uptime_secs":…, "last_reindex_ms":…, "epoch":…}`. The
`epoch` counter increments on every successful reindex — useful for
cache invalidation in downstream consumers.

### `{"cmd":"stats"}`

Returns `{"files":…, "symbols":…, "refs":…, "chunks":…}`. Cheap,
lock-free read off the SQLite index.

### `{"cmd":"reindex"}`

Synchronous hash-incremental `IndexPipeline::run_structural` on a
worker thread. Returns
`{"ok":true,"reindexed":N,"removed":M,"duration_ms":…,"epoch":…}`.

Two concurrent reindex calls produce **one structured error** rather
than fighting at the SQLite layer:

```json
{"ok":false,"error":"index lock contention"}
```

The contention is mediated by a `try_lock`'d mutex — the loser gets
the error and the winner runs to completion.

### `{"cmd":"logs","tail":N}`

Last N lines of `daemon.log`, streamed via a bounded `VecDeque` so
memory stays O(N) regardless of log size. Defaults to 50 when called
through the CLI.

### `{"cmd":"mcp"}` (V2)

Switches the connection into MCP stdio mode. After the daemon reads
this single line, the rest of the socket becomes a JSON-RPC channel
served by an `rmcp` instance with the same 7-tool surface as
`abyss mcp`. The verb itself has **no JSON response envelope** —
the very next bytes are the MCP `initialize` request/response.

```sh
( printf '{"cmd":"mcp"}\n'
  printf '%s\n' '{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
) | nc -U .code-abyss/daemon.sock
# → {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"rmcp","version":"1.7.0"}}}
```

Each MCP-mode connection gets its own
[`Repository::open_read_only`](../../src/storage/repo.rs) handle —
WAL allows N concurrent readers alongside the watcher's single
writer, so multiple MCP clients sharing one daemon don't contend at
the SQLite layer.

`abyss mcp --via-daemon` is the user-facing client. The pre-edit
hook deliberately stays on the direct-SQLite path: a socket hop
would regress its sub-12ms budget for no agent-visible benefit.

## What's deferred to V2.1

WAL-pool tuning, multi-reader stress benchmarks, and an embedder
reference shared between daemon and per-connection MCP handlers are
V2.1. V2 ships the verb + flag + per-connection read-only handle as
the minimum useful surface.
