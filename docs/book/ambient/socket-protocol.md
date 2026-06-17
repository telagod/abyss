# Socket protocol

The V1.5 daemon socket speaks newline-delimited JSON over a Unix
socket at `.code-abyss/daemon.sock`. Four verbs.

```sh
echo '{"cmd":"ping"}'     | nc -U .code-abyss/daemon.sock
echo '{"cmd":"stats"}'    | nc -U .code-abyss/daemon.sock
echo '{"cmd":"reindex"}'  | nc -U .code-abyss/daemon.sock
echo '{"cmd":"logs","tail":50}' | nc -U .code-abyss/daemon.sock
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

## What's deferred to V2

Full MCP-over-socket (so the hook, the MCP server, and an editor
extension can share one in-process index) is V2 territory. In V1.5
the pre-edit hook still reads SQLite directly — no daemon round-trip
on the fast path.
