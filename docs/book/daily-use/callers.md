# `abyss callers`

Cross-file caller tracing with confidence scores.

```sh
abyss callers ValidateToken
abyss callers ValidateToken --min-confidence 0       # include guesses
abyss callers ValidateToken --json
```

## `--calls-only` / `--types-only` (v0.5.0)

Before v0.5.0, `callers` silently treated only call refs as callers.
Type refs (TypeScript interfaces, generics, `extends` clauses, etc.)
were invisible — dogfooding vite turned up `callers ViteDevServer` at
**zero hits** when the type genuinely had **71 production users**.

v0.5.0 includes type refs by default. The two new flags let you opt
back into the old behavior or invert it:

```sh
abyss callers ViteDevServer                # both (default; 71 hits on vite)
abyss callers ViteDevServer --calls-only   # call refs only
abyss callers ViteDevServer --types-only   # type refs only
```

MCP's `find_callers` gains a matching `kinds` filter.

## Confidence

Every caller row carries a confidence score in `[0, 1]`. The default
gate is `0.7`. See [Resolver tiers and confidence](../architecture/resolver-tiers.md)
for the per-tier table.

`--min-confidence 0` reveals everything, including the demoted
"possibles" tier — useful for hunting dynamic dispatch or
metaprogrammed call sites that no static analyzer can resolve.
