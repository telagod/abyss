#!/usr/bin/env bash
# Reproducible eval: abyss call-graph resolution vs SCIP ground truth.
#
# Prereqs: abyss, scip (CLI), scip-go on PATH; git, python3.
#   scip:    https://github.com/sourcegraph/scip/releases
#   scip-go: go install github.com/scip-code/scip-go/cmd/scip-go@latest
set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
CORPUS="$EVAL_DIR/corpus"
mkdir -p "$CORPUS"

# repo|clone-url|pinned-ref|indexer
REPOS=(
  "gin|https://github.com/gin-gonic/gin.git|v1.10.0|scip-go"
)

for entry in "${REPOS[@]}"; do
  IFS='|' read -r name url ref indexer <<<"$entry"
  dir="$CORPUS/$name"

  if [ ! -d "$dir" ]; then
    echo "--- cloning $name @ $ref" >&2
    git clone -q --depth 1 --branch "$ref" "$url" "$dir"
  fi

  cd "$dir"

  if [ ! -f scip.json ]; then
    echo "--- ground truth: $indexer on $name" >&2
    case "$indexer" in
      scip-go) go mod download && scip-go ;;
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
