# L0 architectural coordinates

abyss classifies every indexed file into an architectural layer (`api`,
`domain`, `infra`, `util`, `entry`, `test`, `config`, `vendor`,
`generated`) using a built-in path-segment dictionary. This is the
"L0" coordinate that surfaces on the `where:` line of the pre-edit
card and powers `abyss where`.

The full reference — built-in vocabulary, the `.code-abyss/arch.toml`
override system, fusion semantics, and tolerant-load policy — lives
in the [arch layers reference](https://github.com/telagod/abyss/blob/main/docs/ARCH-LAYERS.md).

The short version:

```toml
[layers]
# Additional path-segment rules, layered ON TOP of the defaults.
# weight is optional (defaults to 0.5); higher weight wins ties with
# the built-in 0.4 dictionary hints.
graph    = { layer = "infra", weight = 0.6 }
temporal = { layer = "infra", weight = 0.6 }
indexer  = { layer = "infra", weight = 0.6 }

[ignore]
# Regex patterns that, if matched against the relative path, skip
# arch inference entirely for that file. No arch_facts row gets
# written.
patterns = ["^vendor/", "^node_modules/", "\\.generated\\."]
```

Reindex any time you edit `arch.toml` — overrides apply at index time,
not at read time.

```sh
abyss index                               # picks up the new arch.toml
abyss where src/graph/languages/go.rs     # check the resulting layer
```
