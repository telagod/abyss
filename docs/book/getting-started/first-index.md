# First index

From inside any project directory:

```sh
abyss index                 # build structural index (~seconds)
abyss map                   # hotspots + coupling overview
abyss context src/auth.go   # full context before editing a file
abyss impact ValidateToken  # blast radius of changing a symbol
```

The index lives at `.code-abyss/index.db` in your workspace. It's
hash-incremental — re-running `abyss index` after a small edit only
re-parses the changed files.

Every command takes `--json` for machine consumption.

## Language coverage

Caller tracing & impact analysis (reference extraction):

| Language | Calls | Type refs | Imports |
|----------|-------|-----------|---------|
| Go | yes | yes | yes |
| Rust | yes | yes | yes |
| TypeScript / TSX | yes | yes | yes |
| JavaScript | yes | yes | yes |
| Python | yes | yes | yes |
| Java | yes | yes | yes |
| C | yes | yes | yes |
| C++ | yes | yes | yes |

Symbol indexing & search additionally cover JSON, TOML, YAML, Bash,
HTML, CSS.

## Workspace safety

abyss refuses to index `$HOME` or `/` (no `.git` + huge file count
triggers a circuit breaker). The index is bounded by hand-written code,
not by repo size — generated files keep symbols but skip ref extraction
unless you pass `--index-generated`.
