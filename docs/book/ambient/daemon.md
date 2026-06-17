# `abyss daemon start --detach` — background

The background daemon (Unix only) survives the shell that launched it.
A pidfile-locked, Unix-socket-fronted process that reindexes on file
events.

```sh
abyss daemon start --detach        # double-fork + setsid, returns once pidfile is claimed
abyss daemon start &               # alternative: shell-backgrounded (no double-fork)
abyss daemon status                # prints pid, uptime, last reindex; exit 1 if not running
abyss daemon stop                  # SIGTERM the recorded pid, wait ≤5s for cleanup
abyss daemon logs --tail 50        # tail .code-abyss/daemon.log (default N=50)
```

## V1.5 detach semantics

`--detach` does a proper **double-fork + `setsid`** so the daemon
survives the shell. Stdin is closed; stdout/stderr land in
`.code-abyss/daemon.log`. The parent returns once the grandchild has
claimed the pidfile (≤500 ms), so `daemon start --detach && daemon status`
chains cleanly.

## Files on disk

- `.code-abyss/daemon.pid` — pidfile, flock-locked. One daemon per
  workspace.
- `.code-abyss/daemon.sock` — Unix socket for operator commands.
- `.code-abyss/daemon.log` — stdout/stderr of the detached process.

## Why both `watch` and `daemon`?

`abyss watch` is the simple foreground flavor — you see the events,
you `Ctrl-C` to stop. `abyss daemon` is what your editor or your CI
worker keeps running in the background. The watcher implementation is
shared.

For the wire protocol, see [Socket protocol](./socket-protocol.md).
