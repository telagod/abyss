# `abyss where`, `context`, `impact`

These are the three "tell me about this thing" commands.

## `abyss where <path>`

Prints the architectural layer of a file. Useful for sanity-checking
the dictionary and any `.code-abyss/arch.toml` overrides you've added.

```sh
abyss where src/auth/login.go
# api  (handler, route)
```

## `abyss context <path>`

Everything an agent needs before editing a file: callers, deps, risk
score, coupling, top symbols. Roughly the same content as the pre-edit
card but printed for humans.

```sh
abyss context src/auth/login.go
abyss context src/auth/login.go --json   # machine-readable
```

## `abyss impact <symbol>`

Blast-radius analysis. Walks the call graph outward from `<symbol>`,
counts direct and transitive callers, flags uncovered paths (callers
not exercised by any test file), and rolls everything into a risk
score 0–10.

```sh
abyss impact ValidateToken
# impact: ValidateToken  direct=17  transitive=521  tests=3  uncovered=319  risk=8.5/10
#   ⚠ high blast radius
#   ⚠ 319 paths without test coverage
```

`--min-confidence 0.7` is the default gate — change it with
`--min-confidence 0` to include demoted possibilities.
