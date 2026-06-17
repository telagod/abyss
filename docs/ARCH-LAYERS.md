# Arch layers — project overrides via `.code-abyss/arch.toml`

abyss classifies every indexed file into an architectural layer (`api`,
`domain`, `infra`, `util`, `entry`, `test`, `config`, `vendor`, `generated`)
using a built-in path-segment dictionary. The classification feeds the
`abyss where` command and the `where:` line of the pre-edit
`<abyss-card>` an agent reads before editing.

The built-in dictionary covers ~25 generic vocabulary buckets (auth, handler,
controller, repository, middleware, route, service, queue, event, cache,
scheduler, worker, validator, log, metric, db, etc.). Projects that use
domain-specific directory names get `layer = "unknown"` until you teach the
dictionary about them.

Drop a `.code-abyss/arch.toml` at the workspace root to do that:

```toml
[layers]
# additional path-segment rules, layered ON TOP of the defaults.
# weight is optional (defaults to 0.5); higher weight wins ties with the
# built-in 0.4 dictionary hints.
graph    = { layer = "infra", weight = 0.6 }
temporal = { layer = "infra", weight = 0.6 }
indexer  = { layer = "infra", weight = 0.6 }

[ignore]
# regex patterns that, if matched against the relative path, skip arch
# inference entirely for that file. No arch_facts row gets written.
patterns = ["^vendor/", "^node_modules/", "\\.generated\\."]
```

## Semantics

- **Keys are path segments, not regex.** Each `[layers]` key is automatically
  anchored to a path-segment boundary, case-insensitive: writing `graph`
  matches `src/graph/foo.go` and `graph.go` but not `paragraph_writer.go`.
- **Layered, not replacing.** User rules are appended to the default
  dictionary signal vector. The fusion layer picks the highest-weight rule;
  if you want to override `service → domain` (default 0.4) with
  `service → infra`, set weight ≥ 0.5.
- **Ignore patterns are real regex** matched against the forward-slash
  normalized relative path. Use `^` to anchor at the workspace root.
- **Tolerant load.** Missing file → silent no-op. Malformed TOML → one
  `WARN` log line and silent fallback to defaults. Malformed regex inside a
  rule → that rule is skipped, the rest of the file still loads. The arch
  pipeline must never crash on a user config typo.

## Verifying an override

```sh
abyss index                               # picks up the new arch.toml
abyss where src/graph/languages/go.rs     # check the resulting layer
```

Reindex any time you edit `arch.toml` — the overrides are applied at index
time, not at read time.
