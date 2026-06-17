#!/usr/bin/env bash
# eval/hostile-concurrent.sh — WAL reader+writer race bench for the abyss
# hook + indexer.
#
# Purpose: hostile.sh measures sequential-ish hook bursts. Real ambient mode
# fires PreToolUse + PostToolUse hooks CONCURRENTLY from the same agent while
# a watcher reindex (writer) may also be running. SQLite WAL handles
# N readers + 1 writer, but the bench has never measured the race.
#
# What this script does:
#   1. Builds release abyss + indexes the abyss self-repo once.
#   2. Fires HOOK_COUNT (default 50) `abyss hook pre-edit` invocations
#      concurrently via xargs -P PARALLELISM against the SAME index DB.
#   3. Simultaneously runs WRITER_ROUNDS (default 5) `abyss index --force`
#      in a serial background loop — that's the WAL writer side.
#   4. Captures:
#        * how many hook invocations fail (target: 0)
#        * how many writer rounds completed
#        * max + p95 + p99 hook latency observed during writer contention
#        * total wall time of the race
#
# Output:
#   * eval/hostile-concurrent-results.json — machine-readable
#   * appended "Concurrent (<UTC date>)" section in eval/hostile-baseline.txt
#
# Usage:
#   eval/hostile-concurrent.sh [repo_path]   # defaults to the abyss repo
#
# Tunables (env vars):
#   HOOK_COUNT      number of concurrent pre-edit invocations  (default 50)
#   PARALLELISM     xargs -P value                              (default 8)
#   WRITER_ROUNDS   background `abyss index --force` rounds     (default 5)

set -euo pipefail

REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$EVAL_DIR/.." && pwd)"
RESULTS_JSON="$EVAL_DIR/hostile-concurrent-results.json"
BASELINE_TXT="$EVAL_DIR/hostile-baseline.txt"

HOOK_COUNT="${HOOK_COUNT:-50}"
PARALLELISM="${PARALLELISM:-8}"
WRITER_ROUNDS="${WRITER_ROUNDS:-5}"

# ---- prereqs ---------------------------------------------------------------
need() { command -v "$1" >/dev/null 2>&1 || { echo "missing dep: $1" >&2; exit 1; }; }
need jq
need cargo

if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3_query() { sqlite3 "$1" "$2"; }
elif command -v python3 >/dev/null 2>&1; then
  echo "[hostile-concurrent] sqlite3 CLI not found — using python3 stdlib fallback" >&2
  sqlite3_query() {
    python3 -c '
import sqlite3, sys
db, sql = sys.argv[1], sys.argv[2]
con = sqlite3.connect(db)
for row in con.execute(sql):
    print("|".join(str(c) if c is not None else "" for c in row))
' "$1" "$2"
  }
else
  echo "missing dep: sqlite3 (CLI or python3)" >&2; exit 1
fi

REPO="$(cd "$REPO" && pwd)"
DB="$REPO/.code-abyss/index.db"

echo "[hostile-concurrent] repo=$REPO"
echo "[hostile-concurrent] hook_count=$HOOK_COUNT parallelism=$PARALLELISM writer_rounds=$WRITER_ROUNDS"

# ---- phase 1: build + index ------------------------------------------------
echo "[hostile-concurrent] phase 1: cargo build --release"
( cd "$ROOT" && cargo build --release --quiet )
ABYSS="$ROOT/target/release/abyss"
[[ -x "$ABYSS" ]] || { echo "build did not produce $ABYSS" >&2; exit 1; }

echo "[hostile-concurrent] phase 1: indexing $REPO (clean DB)"
rm -f "$DB"
"$ABYSS" --workspace "$REPO" index >/dev/null

# ---- phase 2: pick a pool of editable files -------------------------------
SUPPORTED_LIKE="path LIKE '%.go' OR path LIKE '%.rs' OR path LIKE '%.ts'
   OR path LIKE '%.tsx' OR path LIKE '%.js' OR path LIKE '%.jsx'
   OR path LIKE '%.mjs' OR path LIKE '%.cjs' OR path LIKE '%.py'
   OR path LIKE '%.pyi' OR path LIKE '%.java' OR path LIKE '%.c'
   OR path LIKE '%.h' OR path LIKE '%.cpp' OR path LIKE '%.cc'
   OR path LIKE '%.cxx' OR path LIKE '%.hpp'"

TMPDIR_RUN="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_RUN"' EXIT

POOL="$TMPDIR_RUN/pool"
sqlite3_query "$DB" "SELECT path FROM files WHERE $SUPPORTED_LIKE;" > "$POOL"
POOL_SIZE=$(wc -l < "$POOL" | tr -d ' ')
echo "[hostile-concurrent] pool size: $POOL_SIZE files"
[[ "$POOL_SIZE" -gt 0 ]] || { echo "no supported files indexed — abort" >&2; exit 1; }

# Sample HOOK_COUNT lines from $POOL with replacement.
pick_random_files() {
  local n="$1"
  awk -v n="$n" -v seed="$RANDOM" '
    BEGIN { srand(seed) }
    { lines[NR]=$0 }
    END {
      if (NR == 0) exit
      for (i = 0; i < n; i++) {
        idx = int(rand() * NR) + 1
        print lines[idx]
      }
    }
  ' "$POOL"
}

FILES="$TMPDIR_RUN/files"
pick_random_files "$HOOK_COUNT" > "$FILES"

# Single hook call. Returns: <elapsed_ns>\t<exit_status>\n
single_call() {
  local rel="$1" repo="$2" abyss="$3"
  local abs="$repo/$rel"
  local payload start end rc
  payload="$(jq -nc --arg p "$abs" '{tool_name:"Edit", tool_input:{file_path:$p}}')"
  touch -m -- "$abs" 2>/dev/null || true
  start=$(date +%s%N)
  printf '%s' "$payload" | "$abyss" --workspace "$repo" hook pre-edit >/dev/null 2>&1
  rc=$?
  end=$(date +%s%N)
  printf '%s\t%s\n' "$((end - start))" "$rc"
}
export -f single_call

# Percentile helper (ns input → ms output).
percentiles_ms() {
  local file="$1"
  sort -n "$file" | awk '
    { a[NR]=$1 }
    END {
      n=NR
      if (n==0) { print "0 0 0 0"; exit }
      i50=int((n-1)*0.50)+1
      i95=int((n-1)*0.95)+1
      i99=int((n-1)*0.99)+1
      printf "%.2f %.2f %.2f %.2f\n", a[i50]/1e6, a[i95]/1e6, a[i99]/1e6, a[n]/1e6
    }
  '
}

# ---- phase 3: background writer loop --------------------------------------
WRITER_LOG="$TMPDIR_RUN/writer.log"
WRITER_COMPLETED_FILE="$TMPDIR_RUN/writer-completed"
: > "$WRITER_COMPLETED_FILE"

writer_loop() {
  local n="$1" abyss="$2" repo="$3" log="$4" done_file="$5"
  local i=0
  for ((i=0; i<n; i++)); do
    if "$abyss" --workspace "$repo" index --force >>"$log" 2>&1; then
      echo "$i" >> "$done_file"
    fi
  done
}
export -f writer_loop

echo "[hostile-concurrent] phase 3: launching writer loop ($WRITER_ROUNDS rounds) in background"
writer_loop "$WRITER_ROUNDS" "$ABYSS" "$REPO" "$WRITER_LOG" "$WRITER_COMPLETED_FILE" &
WRITER_PID=$!

# Give the writer a head start so the very first hook call already sees
# WAL contention. 100ms is plenty for it to claim the writer lock.
sleep 0.1

# ---- phase 4: fan out concurrent hook invocations ------------------------
echo "[hostile-concurrent] phase 4: firing $HOOK_COUNT hook calls (parallelism=$PARALLELISM) against an active writer"
RAW="$TMPDIR_RUN/raw"
: > "$RAW"
RACE_START=$(date +%s%N)
xargs -a "$FILES" -I{} -P "$PARALLELISM" \
  bash -c 'single_call "$@"' _ {} "$REPO" "$ABYSS" >> "$RAW"
RACE_END=$(date +%s%N)
RACE_WALL_MS=$(awk -v s="$RACE_START" -v e="$RACE_END" 'BEGIN { printf "%.2f", (e - s) / 1e6 }')

# Wait for the writer to finish whatever it's mid-round on.
wait "$WRITER_PID" || true

WRITER_DONE=$(wc -l < "$WRITER_COMPLETED_FILE" | tr -d ' ')

# ---- phase 5: aggregate ----------------------------------------------------
LAT_NS="$TMPDIR_RUN/lat-ns"
awk -F'\t' '{print $1}' "$RAW" > "$LAT_NS"
HOOK_TOTAL=$(wc -l < "$RAW" | tr -d ' ')
HOOK_FAIL=$(awk -F'\t' '$2 != 0' "$RAW" | wc -l | tr -d ' ')
HOOK_PASS=$((HOOK_TOTAL - HOOK_FAIL))

read -r LP50 LP95 LP99 LMAX < <(percentiles_ms "$LAT_NS")

TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
HOST="$(uname -srm)"
ABYSS_VER="$("$ABYSS" --version 2>/dev/null | awk '{print $NF}')"

jq -n \
  --arg ts "$TS" \
  --arg repo "$REPO" \
  --arg host "$HOST" \
  --arg version "$ABYSS_VER" \
  --argjson hook_total "$HOOK_TOTAL" \
  --argjson hook_pass "$HOOK_PASS" \
  --argjson hook_fail "$HOOK_FAIL" \
  --argjson parallelism "$PARALLELISM" \
  --argjson writer_rounds_planned "$WRITER_ROUNDS" \
  --argjson writer_rounds_completed "$WRITER_DONE" \
  --argjson lat_p50_ms "$LP50" \
  --argjson lat_p95_ms "$LP95" \
  --argjson lat_p99_ms "$LP99" \
  --argjson lat_max_ms "$LMAX" \
  --argjson race_wall_ms "$RACE_WALL_MS" \
  '{timestamp:$ts, repo:$repo, host:$host, abyss_version:$version,
    parallelism:$parallelism,
    writer:{rounds_planned:$writer_rounds_planned, rounds_completed:$writer_rounds_completed},
    hooks:{total:$hook_total, ok:$hook_pass, fail:$hook_fail},
    hook_latency_ms:{p50:$lat_p50_ms, p95:$lat_p95_ms, p99:$lat_p99_ms, max:$lat_max_ms},
    race_wall_ms:$race_wall_ms}' \
  > "$RESULTS_JSON"

echo
echo "[hostile-concurrent] results:"
echo "  hooks      total=$HOOK_TOTAL ok=$HOOK_PASS fail=$HOOK_FAIL"
echo "  writer     planned=$WRITER_ROUNDS completed=$WRITER_DONE"
echo "  latency    p50=${LP50}ms p95=${LP95}ms p99=${LP99}ms max=${LMAX}ms"
echo "  race wall  ${RACE_WALL_MS}ms"
echo
echo "[hostile-concurrent] wrote $RESULTS_JSON"

# ---- phase 6: append to baseline -------------------------------------------
TS_DATE="$(date -u +%Y-%m-%d)"
cat >> "$BASELINE_TXT" <<BASELINE_EOF

Concurrent ($TS_DATE)
==================================================================

WAL reader+writer race on $REPO. $HOOK_COUNT concurrent
\`abyss hook pre-edit\` calls (parallelism=$PARALLELISM) racing $WRITER_ROUNDS
background \`abyss index --force\` writer rounds. SQLite WAL is supposed
to handle N readers + 1 writer cleanly; this run measures the actual
behaviour under operator-grade contention.

  hooks      total=$HOOK_TOTAL ok=$HOOK_PASS fail=$HOOK_FAIL
  writer     planned=$WRITER_ROUNDS completed=$WRITER_DONE
  latency    p50=${LP50}ms p95=${LP95}ms p99=${LP99}ms max=${LMAX}ms
  race wall  ${RACE_WALL_MS}ms
BASELINE_EOF

echo "[hostile-concurrent] appended summary to $BASELINE_TXT"
