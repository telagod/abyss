#!/usr/bin/env bash
# Reproducible eval: abyss call-graph resolution vs SCIP ground truth.
#
# Prereqs: abyss, scip (CLI), scip-go on PATH; git, python3, node.
#   scip:    https://github.com/sourcegraph/scip/releases
#   scip-go: go install github.com/scip-code/scip-go/cmd/scip-go@latest
#   scip-typescript / scip-python:
#            npm install -g @sourcegraph/scip-typescript @sourcegraph/scip-python
#   rust-analyzer: rustup component add rust-analyzer
#   scip-clang: https://github.com/sourcegraph/scip-clang/releases (+ cmake)
set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
CORPUS="$EVAL_DIR/corpus"
mkdir -p "$CORPUS"

# ── Version sanity guard ───────────────────────────────────────────────────
# Log every indexer's --version to stderr so future eval runs document what
# they actually ran against. SCIP indexers move silently (a scip-python
# bump between v0.3.6 and v0.4.0 added 16 truth pairs to click, shifting the
# baseline 98.7/94.6 → 97.9/93.0 with zero abyss code change). We never
# error on mismatch here — pins live in eval/setup-indexers.sh; this is
# just the audit trail.
echo "--- indexer versions (audit trail; pins in setup-indexers.sh)" >&2
for tool in scip scip-go scip-typescript scip-python rust-analyzer scip-clang; do
  if command -v "$tool" >/dev/null 2>&1; then
    # scip-go's --version writes to stdout w/o newline; others vary. Trim
    # to first line so multi-line banners don't pollute the log.
    ver="$("$tool" --version 2>&1 | head -1 | tr -d '\r' || true)"
    printf '    %-18s %s\n' "$tool" "${ver:-(no --version output)}" >&2
  else
    printf '    %-18s (not on PATH)\n' "$tool" >&2
  fi
done
echo >&2

# repo|clone-url|pinned-ref|indexer
REPOS=(
  "gin|https://github.com/gin-gonic/gin.git|v1.10.0|scip-go"
  "hono|https://github.com/honojs/hono.git|v4.6.14|scip-typescript"
  "click|https://github.com/pallets/click.git|8.1.8|scip-python"
  "ripgrep|https://github.com/BurntSushi/ripgrep.git|14.1.1|rust-analyzer"
  # dogfood: abyss itself, pinned to the commit the numbers were taken at
  "abyss|https://github.com/telagod/abyss.git|8099aeb|rust-analyzer"
  "cmark|https://github.com/commonmark/cmark.git|0.31.1|scip-clang"
)

for entry in "${REPOS[@]}"; do
  IFS='|' read -r name url ref indexer <<<"$entry"
  if ! command -v "$indexer" >/dev/null 2>&1; then
    echo "--- skip $name: indexer '$indexer' not on PATH" >&2
    continue
  fi
  dir="$CORPUS/$name"

  if [ ! -d "$dir" ]; then
    echo "--- cloning $name @ $ref" >&2
    # tags/branches take the cheap path; bare commit shas need a full clone
    git clone -q --depth 1 --branch "$ref" "$url" "$dir" 2>/dev/null || {
      git clone -q "$url" "$dir"
      git -C "$dir" checkout -q "$ref"
    }
  fi

  cd "$dir"

  if [ ! -f scip.json ]; then
    echo "--- ground truth: $indexer on $name" >&2
    case "$indexer" in
      scip-go) go mod download && scip-go ;;
      scip-typescript)
        # --ignore-scripts: corpus deps are only needed for type info
        npm install --ignore-scripts --no-audit --no-fund >&2
        scip-typescript index >&2
        ;;
      scip-python)
        scip-python index . --project-name "$name" >&2
        ;;
      rust-analyzer)
        rust-analyzer scip . --output index.scip >&2
        ;;
      scip-clang)
        cmake -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >&2
        scip-clang --compdb-path=build/compile_commands.json >&2
        ;;
      *) echo "unknown indexer: $indexer" >&2; exit 1 ;;
    esac
    scip print --json index.scip > scip.json
  fi

  echo "--- abyss index on $name" >&2
  rm -rf .code-abyss
  abyss index

  echo "--- compare" >&2
  python3 "$EVAL_DIR/compare.py" "$dir"
  echo
done
