#!/usr/bin/env bash
# Agent A/B v2: does pre-edit caller context change agent behavior on a REAL
# codebase (gin, 102 files)?
#
# Three arms:
#   control — task prompt only
#   grep    — task + raw `grep -rn "<symbol>(" --include=*.go .` output (placebo:
#             separates "any caller hint helps" from "structured graph helps")
#   abyss   — task + verbatim `abyss context <target>` output
#
# Tasks are signature mutations on internal helpers with cross-file callers.
# Agents get Read/Grep/Glob/Edit only — no shell, no compiler feedback. The
# compile gate at grading time is the objective judge: miss one caller and
# `go test ./...` fails. No "keep the system consistent" priming in prompts.
#
# Usage: run.sh [reps]   (default 2 → 4 tasks × 3 arms × 2 reps = 24 trials)
set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
GIN="$EVAL_DIR/../corpus/gin"
WORK=/tmp/abyss-ab2
REPS="${1:-2}"
MODEL="${ABYSS_AB_MODEL:-claude-haiku-4-5}"
CONCURRENCY="${ABYSS_AB_JOBS:-6}"

command -v claude >/dev/null || { echo "claude CLI required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
[ -d "$GIN" ] || { echo "gin corpus missing — run eval/run.sh first" >&2; exit 1; }

mkdir -p "$WORK"

# ── Precompute arm contexts on the pristine corpus ──
ABYSS_BIN="${ABYSS_BIN:-abyss}"
( cd "$GIN" && [ -f .code-abyss/index.db ] || "$ABYSS_BIN" index >/dev/null 2>&1 )

ntasks=$(jq length "$EVAL_DIR/tasks.json")
for i in $(seq 0 $((ntasks - 1))); do
  id=$(jq -r ".[$i].id" "$EVAL_DIR/tasks.json")
  target=$(jq -r ".[$i].target" "$EVAL_DIR/tasks.json")
  symbol=$(jq -r ".[$i].symbol" "$EVAL_DIR/tasks.json")
  ( cd "$GIN" && "$ABYSS_BIN" context "$target" > "$WORK/$id.abyss.txt" 2>/dev/null )
  ( cd "$GIN" && grep -rn "${symbol}(" --include="*.go" . > "$WORK/$id.grep.txt" )
done

# ── Launch trials ──
run_trial() {
  local id="$1" arm="$2" rep="$3"
  local trial="$WORK/$id-$arm-r$rep"
  rm -rf "$trial"
  rsync -a --exclude .code-abyss --exclude index.scip --exclude scip.json \
    "$GIN/" "$trial/"

  local task
  task=$(jq -r ".[] | select(.id == \"$id\") | .task" "$EVAL_DIR/tasks.json")
  local prompt="You are working in the gin web framework Go repository (current directory).

Task: $task

Rules: do not modify any *_test.go files. You cannot run shell commands, builds, or tests; make all changes with the editing tools."

  case "$arm" in
    grep)
      prompt="$prompt

Output of \`grep -rn '$(jq -r ".[] | select(.id == \"$id\") | .symbol" "$EVAL_DIR/tasks.json")(' --include='*.go' .\`:
$(cat "$WORK/$id.grep.txt")" ;;
    abyss)
      prompt="$prompt

Output of \`abyss context $(jq -r ".[] | select(.id == \"$id\") | .target" "$EVAL_DIR/tasks.json")\` (code-graph callers for the file you are changing):
$(cat "$WORK/$id.abyss.txt")" ;;
  esac

  (
    cd "$trial"
    start=$(date +%s%3N)
    claude -p "$prompt" \
      --allowedTools "Read,Grep,Glob,Edit" \
      --model "$MODEL" \
      --max-turns 80 \
      --output-format json > .result.json 2> .stderr || true
    end=$(date +%s%3N)
    echo $((end - start)) > .wallclock_ms
  )
  echo "done: $id-$arm-r$rep" >&2
}

pids=()
for rep in $(seq 1 "$REPS"); do
  for i in $(seq 0 $((ntasks - 1))); do
    id=$(jq -r ".[$i].id" "$EVAL_DIR/tasks.json")
    for arm in control grep abyss; do
      run_trial "$id" "$arm" "$rep" &
      pids+=($!)
      while [ "$(jobs -rp | wc -l)" -ge "$CONCURRENCY" ]; do sleep 2; done
    done
  done
done
wait
echo "all trials complete → $WORK" >&2
