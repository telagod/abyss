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

## Module labels (v0.5.0)

`abyss where` also prints a `module=...` field. Modules come out of
graph clustering (Louvain on the import graph), not the layer
dictionary, so they reflect *who imports whom*, not *what kind of file
this is*. v0.5.0 changed how the label is rendered:

- **Boring monorepo prefixes are stripped before label picking.**
  `packages`, `crates`, `libs`, `src`, `lib`, `app`, `apps`,
  `services`, `modules`, `internal` no longer dominate the modal
  segment. Vite's modules went from `p-N` to `vite-N`, helix's went
  from `helix--N` toward crate-name labels.
- **Cross-cutting communities render as `cluster-N`.** When no single
  path segment claims ≥ 50% of the module's members, the labeller
  emits `cluster-N` instead of the internal `mixed:{seg}+` bookkeeping
  that used to leak into output.
- **Genuinely unlabellable modules render as `unlabelled-N`.** Empty
  centroid path, no winning segment — say so honestly rather than
  appending a digit to a fragment.

The principle: a label that *looks* informative but isn't costs more
trust than an honest fallback. See
[docs/PRINCIPLES.md](PRINCIPLES.md) §5.

## FAQ

**Why is my module labeled `cluster-3`?**
The cluster is a cross-cutting community — files from several
different top-level directories ended up co-imported strongly enough
that Louvain merged them. No single directory segment owns ≥ 50% of
the members, so the labeller falls back to `cluster-N` rather than
inventing a label out of a minority segment. To rename it, drop a
`[layers]` rule that gives the dominant subsystem a higher weight, or
restructure the imports so the community has a clear centroid.

**Why is everything labeled `unknown`?**
The built-in layer dictionary is tuned for Western/web/service
vocabulary (`auth`, `handler`, `controller`, `repository`,
`middleware`, `route`, `service`, `queue`, `event`, `cache`,
`scheduler`, `worker`, `validator`, `log`, `metric`, `db`, …). If your
project uses non-Western terms (Chinese / Japanese / Korean directory
names) or non-web vocabulary (`view`, `term`, `tui`, `core`, `lsp`,
`dap`, `vcs`, `stdx`, `loader` — the helix-editor case), the
dictionary contributes nothing and you get 80%+ `unknown` with
`conf=0`. Fix: drop a `.code-abyss/arch.toml` like the example above,
listing the segment vocabulary specific to your project and the
layer each maps to. Layer signal is dead metadata until you teach the
dictionary about your repo's words.

**Why does my hub file end up in a satellite module like `tests`?**
Module clustering is driven by edge density. If hundreds of tests
import one production file, the production file can get pulled into
the test community by sheer connectivity. This is a known limitation
(see FastAPI dogfood B3,
[docs/dogfood/fastapi-2026-06-17.md](dogfood/fastapi-2026-06-17.md)).
Workaround: a path-prefix override in `arch.toml` cannot move a file
across modules — modules are a graph property, not a path property —
but you can rely on the topological signals (`role=bridge`,
`centrality`, `in/out`) on the same `where` line, which are not
affected by clustering and remain the load-bearing answer to "is this
a hub?".

**Why did `module=` change between v0.4.0 and v0.5.0?**
v0.4.0 emitted `p-18` / `helix--8` / `mixed:create-vite+` — these were
internal labeller state leaking out. v0.5.0 renders the same
underlying clusters as `vite-18` / `helix-view` / `cluster-N`. Module
ids are stable across reindexes of the same content; only the
rendered label string changed.
