# `abyss watch` — foreground

`abyss watch` is the foreground flavor of ambient reindex. It
subscribes to file-system events and triggers a hash-incremental
reindex on save. The 150ms debounce coalesces editor write bursts
(atomic-rename saves, formatter passes) into one update.

```sh
abyss watch                      # default 150ms debounce
abyss watch --debounce-ms 300    # tune for slower disks / heavier formatters
```

`abyss watch` is an alias of `abyss daemon start --foreground`. It
runs in the foreground, prints reindex events as they happen, and
exits when you `Ctrl-C`.

For a background, pidfile-locked daemon, see
[`abyss daemon start --detach`](./daemon.md).
