#!/usr/bin/env bash
# eval/hostile.sh — burst-edit latency bench for the abyss agent hook.
#
# Purpose: turn the "ambient is fast" claim into a number. Picks the most
# symbol-dense files in the target repo, fires N concurrent `abyss hook
# pre-edit` calls against them, and reports p50/p95/p99 wall time plus the
# size of the JSON card the hook emits.
#
# Usage:
#   eval/hostile.sh [repo_path]    # defaults to the abyss repo itself
#
# Prereqs: sqlite3, jq, /usr/bin/time (any modern Linux/macOS), cargo.
#
# Output:
#   * human-readable table on stdout
#   * eval/hostile-results.json  — machine-readable raw + aggregates
#   * eval/hostile-card-sample.json — one card body so reviewers can eyeball
#
# Notes:
#   * BURST_SIZES is the list of concurrent invocations to measure (10/100/500).
#   * Concurrency is capped at PARALLELISM (default 4) — matches what a
#     real editor save-storm actually fans out to disk.
#   * Times come from `date +%s%N` (ns) — portable, no hyperfine dependency.
#   * Hook reads stdin once per process; we measure the full
#     fork+exec+stdin+work+exit cycle. That's what the agent sees.

set -euo pipefail

REPO="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$EVAL_DIR/.." && pwd)"
RESULTS_JSON="$EVAL_DIR/hostile-results.json"
CARD_SAMPLE="$EVAL_DIR/hostile-card-sample.json"
PARALLELISM="${PARALLELISM:-4}"
BURST_SIZES="${BURST_SIZES:-10 100 500}"

# ---- prereqs ---------------------------------------------------------------
need() { command -v "$1" >/dev/null 2>&1 || { echo "missing dep: $1" >&2; exit 1; }; }
need sqlite3
need jq
need cargo

REPO="$(cd "$REPO" && pwd)"
DB="$REPO/.code-abyss/index.db"

echo "[hostile] repo=$REPO"
echo "[hostile] parallelism=$PARALLELISM bursts=$BURST_SIZES"

# ---- phase 1: build + index ------------------------------------------------
echo "[hostile] phase 1: cargo build --release"
( cd "$ROOT" && cargo build --release --quiet )
ABYSS="$ROOT/target/release/abyss"
[[ -x "$ABYSS" ]] || { echo "build did not produce $ABYSS" >&2; exit 1; }

echo "[hostile] phase 1: indexing $REPO (clean DB)"
rm -f "$DB"
"$ABYSS" --workspace "$REPO" index >/dev/null

# ---- phase 2: pick hub files ----------------------------------------------
# Most-symbol-dense files are the realistic "hot edit targets" — touch
# `pipeline.rs` and the hook has to think hardest.
echo "[hostile] phase 2: selecting hub files"
HUBS_FILE="$(mktemp)"
trap 'rm -f "$HUBS_FILE"' EXIT
sqlite3 "$DB" \
  "SELECT f.path FROM files f
     JOIN symbols s ON s.file_id = f.id
     GROUP BY f.id
     ORDER BY COUNT(*) DESC
     LIMIT 10;" > "$HUBS_FILE"
HUB_COUNT=$(wc -l < "$HUBS_FILE")
echo "[hostile] top hub files ($HUB_COUNT):"
sed 's/^/    /' "$HUBS_FILE"
[[ "$HUB_COUNT" -gt 0 ]] || { echo "no symbols indexed — abort" >&2; exit 1; }

# ---- phase 3: burst runs ---------------------------------------------------
TMPDIR_RUN="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_RUN"; rm -f "$HUBS_FILE"' EXIT

# Capture one card body for the human reader. Use the densest hub file.
HUB_TOP="$(head -n1 "$HUBS_FILE")"
HUB_TOP_ABS="$REPO/$HUB_TOP"
HUB_TOP_PAYLOAD="$(jq -nc --arg p "$HUB_TOP_ABS" '{tool_name:"Edit", tool_input:{file_path:$p}}')"
echo "[hostile] sampling JSON card for $HUB_TOP"
echo "$HUB_TOP_PAYLOAD" | "$ABYSS" --workspace "$REPO" hook pre-edit --json > "$CARD_SAMPLE" 2>/dev/null || true

# Hook-supported extensions, as one big LIKE filter (REGEXP isn't loaded in
# stock SQLite builds — keep dependencies thin).
SUPPORTED_LIKE="path LIKE '%.go' OR path LIKE '%.rs' OR path LIKE '%.ts'
   OR path LIKE '%.tsx' OR path LIKE '%.js' OR path LIKE '%.jsx'
   OR path LIKE '%.mjs' OR path LIKE '%.cjs' OR path LIKE '%.py'
   OR path LIKE '%.pyi' OR path LIKE '%.java' OR path LIKE '%.c'
   OR path LIKE '%.h' OR path LIKE '%.cpp' OR path LIKE '%.cc'
   OR path LIKE '%.cxx' OR path LIKE '%.hpp'"

# Cache the supported-file pool once so bursts > population are realistic:
# we sample WITH replacement so BURST=500 on a 60-file repo still simulates
# 500 hook invocations (file-system save storms are not deduped).
POOL="$TMPDIR_RUN/pool"
sqlite3 "$DB" "SELECT path FROM files WHERE $SUPPORTED_LIKE;" > "$POOL"
POOL_SIZE=$(wc -l < "$POOL")
echo "[hostile] supported-file pool size: $POOL_SIZE"
[[ "$POOL_SIZE" -gt 0 ]] || { echo "no supported files indexed — abort" >&2; exit 1; }

# Sample `n` lines from $POOL with replacement using awk + $RANDOM. Stable
# across mawk/gawk/BSD awk.
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

# Single hook invocation: prints elapsed_ns<TAB>card_bytes on stdout.
# Stays a function so we can `export -f` for xargs.
single_call() {
  local rel="$1" repo="$2" abyss="$3"
  local abs="$repo/$rel"
  local payload card_bytes start end
  payload="$(jq -nc --arg p "$abs" '{tool_name:"Edit", tool_input:{file_path:$p}}')"
  # Refresh mtime so the hook treats it as a real save.
  touch -m -- "$abs" 2>/dev/null || true
  start=$(date +%s%N)
  card_bytes=$(printf '%s' "$payload" | "$abyss" --workspace "$repo" hook pre-edit 2>&1 >/dev/null | wc -c | tr -d ' ')
  end=$(date +%s%N)
  printf '%s\t%s\n' "$((end - start))" "$card_bytes"
}
export -f single_call

# p50/p95/p99 from an ns list. Sorts via `sort -n` (portable, no gawk needed
# because mawk/awk don't ship asort). Echoes "p50 p95 p99" in ms (2dp).
percentiles_ms() {
  local file="$1"
  sort -n "$file" | awk '
    { a[NR]=$1 }
    END {
      n=NR
      if (n==0) { print "0 0 0"; exit }
      i50=int((n-1)*0.50)+1
      i95=int((n-1)*0.95)+1
      i99=int((n-1)*0.99)+1
      printf "%.2f %.2f %.2f\n", a[i50]/1e6, a[i95]/1e6, a[i99]/1e6
    }
  '
}

percentiles_int() {
  local file="$1"
  sort -n "$file" | awk '
    { a[NR]=$1 }
    END {
      n=NR
      if (n==0) { print "0 0 0"; exit }
      i50=int((n-1)*0.50)+1
      i95=int((n-1)*0.95)+1
      i99=int((n-1)*0.99)+1
      printf "%d %d %d\n", a[i50], a[i95], a[i99]
    }
  '
}

declare -a JSON_ENTRIES
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

echo
printf '%-12s | %-32s | %s\n' "burst"          "hook latency (ms)"            "card body (bytes)"
printf '%-12s | %-32s | %s\n' "------------" "--------------------------------" "----------------------"

for burst in $BURST_SIZES; do
  files="$TMPDIR_RUN/files-$burst"
  raw="$TMPDIR_RUN/raw-$burst"
  lat_ns="$TMPDIR_RUN/lat-$burst"
  card_b="$TMPDIR_RUN/card-$burst"

  pick_random_files "$burst" > "$files"
  picked=$(wc -l < "$files" | tr -d ' ')
  if [[ "$picked" -eq 0 ]]; then
    echo "[hostile] BURST=$burst skipped: no indexed files match" >&2
    continue
  fi

  # Fan out under bounded parallelism.
  : > "$raw"
  xargs -a "$files" -I{} -P "$PARALLELISM" \
    bash -c 'single_call "$@"' _ {} "$REPO" "$ABYSS" >> "$raw"

  awk -F'\t' '{print $1}' "$raw" > "$lat_ns"
  awk -F'\t' '{print $2}' "$raw" > "$card_b"

  read -r LP50 LP95 LP99 < <(percentiles_ms "$lat_ns")
  read -r CP50 CP95 CP99 < <(percentiles_int "$card_b")

  printf '%-12s | p50=%6sms p95=%6sms p99=%6sms | p50=%5s p95=%5s p99=%5s\n' \
    "BURST=$burst" "$LP50" "$LP95" "$LP99" "$CP50" "$CP95" "$CP99"

  # Build JSON entry.
  ENTRY=$(jq -n \
    --argjson burst "$burst" \
    --argjson picked "$picked" \
    --argjson parallelism "$PARALLELISM" \
    --argjson lat_p50_ms "$LP50" \
    --argjson lat_p95_ms "$LP95" \
    --argjson lat_p99_ms "$LP99" \
    --argjson card_p50_bytes "$CP50" \
    --argjson card_p95_bytes "$CP95" \
    --argjson card_p99_bytes "$CP99" \
    '{burst:$burst, picked:$picked, parallelism:$parallelism,
      hook_latency_ms:{p50:$lat_p50_ms, p95:$lat_p95_ms, p99:$lat_p99_ms},
      card_bytes:{p50:$card_p50_bytes, p95:$card_p95_bytes, p99:$card_p99_bytes}}')
  JSON_ENTRIES+=("$ENTRY")
done

# ---- phase 4: persist ------------------------------------------------------
HOST="$(uname -srm)"
ABYSS_VER="$("$ABYSS" --version 2>/dev/null | awk '{print $NF}')"

if [[ "${#JSON_ENTRIES[@]}" -eq 0 ]]; then
  echo "[hostile] no measurements collected" >&2
  exit 1
fi

printf '%s\n' "${JSON_ENTRIES[@]}" | jq -s \
  --arg ts "$TS" \
  --arg repo "$REPO" \
  --arg host "$HOST" \
  --arg version "$ABYSS_VER" \
  '{timestamp:$ts, repo:$repo, host:$host, abyss_version:$version, runs:.}' \
  > "$RESULTS_JSON"

echo
echo "[hostile] wrote $RESULTS_JSON"
echo "[hostile] wrote $CARD_SAMPLE (first ${HUB_TOP} card body)"
