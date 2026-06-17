# Installing

abyss ships as a single static binary. Pick the install method that
matches your environment.

## One-line installer (Linux, macOS — x86_64 + arm64)

```sh
# prebuilt binary with source-build fallback
curl -fsSL https://raw.githubusercontent.com/telagod/abyss/main/install.sh | bash

# mirror, for networks where raw.githubusercontent.com is unreachable
curl -fsSL https://cdn.jsdelivr.net/gh/telagod/abyss@main/install.sh | bash
```

## Package managers

```sh
# via npm (the wrapper downloads the prebuilt binary on install)
npm install -g @code-abyss/cli

# via cargo (binstall fetches the prebuilt binary; install builds from source)
cargo binstall code-abyss   # or: cargo install code-abyss
```

## Windows

Use `npm install -g @code-abyss/cli`, `cargo binstall code-abyss`, or
grab the prebuilt `.zip` from
[GitHub Releases](https://github.com/telagod/abyss/releases).

## From source

```sh
git clone https://github.com/telagod/abyss
cd abyss
./install.sh --from-source
```

## Build flavors

| Build | Contents | Size |
|-------|----------|------|
| default (slim) | call graph + temporal + fulltext + MCP | ~18M |
| `--features semantic` | + embedding-based semantic search (fastembed/ONNX) | ~43M |

Slim is the right default. Add `semantic` only if you want fuzzy
natural-language search on top of the structural index.
