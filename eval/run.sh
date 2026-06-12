#!/usr/bin/env bash
# Reproducible eval: abyss call-graph resolution vs SCIP ground truth.
#
# Prereqs: abyss, scip (CLI), scip-go on PATH; git, python3, node.
#   scip:    https://github.com/sourcegraph/scip/releases
#   scip-go: go install github.com/scip-code/scip-go/cmd/scip-go@latest
#   scip-typescript / scip-python:
#            npm install -g @sourcegraph/scip-typescript @sourcegraph/scip-python
#   rust-analyzer: rustup component add rust-analyzer
set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
CORPUS="$EVAL_DIR/corpus"
mkdir -p "$CORPUS"

# repo|clone-url|pinned-ref|indexer
REPOS=(
  "gin|https://github.com/gin-gonic/gin.git|v1.10.0|scip-go"
  "hono|https://github.com/honojs/hono.git|v4.6.14|scip-typescript"
  "click|https://github.com/pallets/click.git|8.1.8|scip-python"
  "ripgrep|https://github.com/BurntSushi/ripgrep.git|14.1.1|rust-analyzer"
  # dogfood: abyss itself, pinned to the commit the numbers were taken at
  "abyss|https://github.com/telagod/abyss.git|8099aeb|rust-analyzer"
)

for entry in "${REPOS[@]}"; do
  IFS='|' read -r name url ref indexer <<<"$entry"
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
