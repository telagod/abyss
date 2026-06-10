#!/usr/bin/env bash
# Grade A/B v2 trials. Verdict OK iff:
#   applied  — the target signature actually changed (anti "did nothing")
#   tests    — `go test ./...` green (compile gate: every missed caller fails)
#   testfile — no *_test.go modified (git-tracked)
# Also extracts num_turns / duration / cost from the headless-session result.
set -uo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
WORK=/tmp/abyss-ab2

printf "%-18s %-8s %-6s %-9s %-8s %6s %8s %9s\n" \
  trial applied tests testfiles verdict turns wall_s cost_usd

for d in "$WORK"/t*-*-r*/; do
  name=$(basename "$d")
  id="${name%%-*}"
  pattern=$(jq -r ".[] | select(.id == \"$id\") | .applied_pattern" "$EVAL_DIR/tasks.json")
  target=$(jq -r ".[] | select(.id == \"$id\") | .target" "$EVAL_DIR/tasks.json")

  cd "$d" || continue

  if grep -qE "$pattern" "$target" 2>/dev/null; then applied=yes; else applied=NO; fi
  if go test ./... >/dev/null 2>&1; then tests=PASS; else tests=FAIL; fi
  if git diff --name-only 2>/dev/null | grep -q '_test\.go$'; then tf=EDITED; else tf=intact; fi

  if [ "$applied" = yes ] && [ "$tests" = PASS ] && [ "$tf" = intact ]; then
    verdict=OK
  else
    verdict=REGRESSION
  fi

  turns=$(jq -r '.num_turns // "?"' .result.json 2>/dev/null || echo "?")
  wall=$(awk '{printf "%.0f", $1/1000}' .wallclock_ms 2>/dev/null || echo "?")
  cost=$(jq -r '.total_cost_usd // "?"' .result.json 2>/dev/null | cut -c1-7)

  printf "%-18s %-8s %-6s %-9s %-8s %6s %8s %9s\n" \
    "$name" "$applied" "$tests" "$tf" "$verdict" "$turns" "$wall" "$cost"
done

echo
echo "── per-arm summary ──"
for arm in control grep abyss; do
  ok=0; total=0; turns=0; wall=0
  for d in "$WORK"/t*-"$arm"-r*/; do
    name=$(basename "$d"); id="${name%%-*}"
    pattern=$(jq -r ".[] | select(.id == \"$id\") | .applied_pattern" "$EVAL_DIR/tasks.json")
    target=$(jq -r ".[] | select(.id == \"$id\") | .target" "$EVAL_DIR/tasks.json")
    cd "$d" || continue
    total=$((total + 1))
    if grep -qE "$pattern" "$target" 2>/dev/null \
       && go test ./... >/dev/null 2>&1 \
       && ! git diff --name-only 2>/dev/null | grep -q '_test\.go$'; then
      ok=$((ok + 1))
    fi
    t=$(jq -r '.num_turns // 0' .result.json 2>/dev/null); turns=$((turns + t))
    w=$(cat .wallclock_ms 2>/dev/null || echo 0); wall=$((wall + w))
  done
  [ "$total" -gt 0 ] || continue
  printf "%-8s OK %d/%d   turns total %d (avg %d)   wall total %ds (avg %ds)\n" \
    "$arm" "$ok" "$total" "$turns" "$((turns / total))" "$((wall / 1000))" "$((wall / total / 1000))"
done
