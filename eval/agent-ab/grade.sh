#!/usr/bin/env bash
# Grade all trials: go test green + integration files untouched.
BASE=~/project/code-abyss-dev/eval/agent-ab/fixture
printf "%-18s %-6s %-10s %s\n" trial tests contract verdict
for d in /tmp/abyss-ab/t*-*/; do
  name=$(basename "$d")
  cd "$d"
  if go test ./... >/dev/null 2>&1; then tests=PASS; else tests=FAIL; fi
  if cmp -s "$d/report/report_test.go" "$BASE/report/report_test.go" \
     && cmp -s "$d/api/handler_test.go" "$BASE/api/handler_test.go"; then
    contract=intact
  else
    contract=VIOLATED
  fi
  if [ "$tests" = PASS ] && [ "$contract" = intact ]; then v=OK; else v=REGRESSION; fi
  printf "%-18s %-6s %-10s %s\n" "$name" "$tests" "$contract" "$v"
done
