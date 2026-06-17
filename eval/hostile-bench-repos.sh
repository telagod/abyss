#!/usr/bin/env bash
# eval/hostile-bench-repos.sh — run hostile.sh across a list of repos and
# aggregate the results into eval/hostile-bigrepos-results.json plus a
# human-readable summary appended to eval/hostile-baseline.txt under a new
# "Big repos (<date>)" section.
#
# v0.5.6: extends the hostile bench from the abyss self-index (82 files)
# to representative real-world Python corpora —
#   * SQLAlchemy (~80K LOC, 688 .py files at /tmp/abyss-dogfood-sqlalchemy)
#   * Django     (~250K LOC, optional — only runs if present)
# so the latency/card-size envelope under burst load is measured on
# something close to a real backend project.
#
# Usage:
#   eval/hostile-bench-repos.sh                       # auto-discover the
#                                                      default dogfood paths
#   eval/hostile-bench-repos.sh path1 [path2 ...]     # explicit list
#
# Behaviour:
#   * For each repo, runs `hostile.sh <repo>` with BURST=10 100 500 (the
#     same envelope as the baseline).
#   * Captures the produced eval/hostile-results.json after each run, tags
#     it with the repo label, and appends to the aggregate JSON.
#   * Repos that don't exist on disk are skipped with a warning — never
#     hard-fails. This is dev-machine eval scripting, not CI.
#   * Writes eval/hostile-bigrepos-results.json (machine-readable) and
#     appends a "Big repos (<UTC date>)" section to eval/hostile-baseline.txt
#     mirroring the existing baseline format.

set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$EVAL_DIR/.." && pwd)"
AGGREGATE="$EVAL_DIR/hostile-bigrepos-results.json"
BASELINE="$EVAL_DIR/hostile-baseline.txt"
HOSTILE="$EVAL_DIR/hostile.sh"
DATE_UTC="$(date -u +%Y-%m-%d)"
TS_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# ---- discovery -------------------------------------------------------------
# Default candidate paths the owner keeps on disk. Each entry is a
# "label:abs-path" pair; the label appears in the aggregate JSON and the
# baseline summary. Caller can override by passing explicit paths.
DEFAULT_CANDIDATES=(
  "sqlalchemy:/tmp/abyss-dogfood-sqlalchemy/sqlalchemy"
  "sqlalchemy-root:/tmp/abyss-dogfood-sqlalchemy"
  "django:/tmp/abyss-dogfood-django"
)

REPOS=()
if [[ $# -gt 0 ]]; then
  # Explicit list: each arg is treated as a path; label = basename.
  for arg in "$@"; do
    if [[ -d "$arg" ]]; then
      REPOS+=("$(basename "$arg"):$arg")
    else
      echo "[hostile-bench-repos] skip (not a directory): $arg" >&2
    fi
  done
else
  # Auto-discover: pick the first DEFAULT_CANDIDATES entry whose path
  # actually exists per label-prefix. Don't index the same project twice
  # if both `sqlalchemy` and `sqlalchemy-root` resolve.
  seen_labels=""
  for entry in "${DEFAULT_CANDIDATES[@]}"; do
    label="${entry%%:*}"
    path="${entry#*:}"
    short_label="${label%%-*}"
    case ":$seen_labels:" in
      *":$short_label:"*) continue ;;
    esac
    if [[ -d "$path" ]]; then
      REPOS+=("$label:$path")
      seen_labels="$seen_labels:$short_label"
    fi
  done
fi

if [[ "${#REPOS[@]}" -eq 0 ]]; then
  echo "[hostile-bench-repos] no candidate repos available on this machine" >&2
  echo "[hostile-bench-repos] expected one of:" >&2
  for entry in "${DEFAULT_CANDIDATES[@]}"; do
    echo "  ${entry#*:}" >&2
  done
  echo "[hostile-bench-repos] skipping bench — no results written" >&2
  exit 0
fi

echo "[hostile-bench-repos] will bench ${#REPOS[@]} repo(s):"
for r in "${REPOS[@]}"; do echo "  ${r%%:*}  ->  ${r#*:}"; done

# ---- run hostile.sh per repo ----------------------------------------------
mkdir -p "$EVAL_DIR"
TMPDIR_AGG="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_AGG"' EXIT

PER_REPO_JSON=()
SUMMARY_LINES=()
SUMMARY_LINES+=("Big repos ($DATE_UTC)")
SUMMARY_LINES+=("==================================================================")

for entry in "${REPOS[@]}"; do
  label="${entry%%:*}"
  path="${entry#*:}"
  echo
  echo "================================================================"
  echo "[hostile-bench-repos] benching $label  ($path)"
  echo "================================================================"

  # hostile.sh writes its result to eval/hostile-results.json — capture
  # immediately into the aggregate dir so the next repo doesn't clobber it.
  if ! "$HOSTILE" "$path"; then
    echo "[hostile-bench-repos] hostile.sh failed for $label — skipping" >&2
    continue
  fi
  cp "$EVAL_DIR/hostile-results.json" "$TMPDIR_AGG/$label.json"
  # Tag the JSON with the label so the aggregate is self-describing.
  jq --arg label "$label" --arg path "$path" \
    '. + {label:$label, repo_path:$path}' \
    "$TMPDIR_AGG/$label.json" > "$TMPDIR_AGG/$label-tagged.json"
  PER_REPO_JSON+=("$TMPDIR_AGG/$label-tagged.json")

  # Build a one-line summary per burst for the baseline text file.
  SUMMARY_LINES+=("")
  SUMMARY_LINES+=("[$label]  ($path)")
  while IFS= read -r line; do
    SUMMARY_LINES+=("  $line")
  done < <(
    jq -r '
      .runs[] |
      "BURST=\(.burst | tostring | . + (" " * (4 - length))) | " +
      "p50=\(.hook_latency_ms.p50 | tostring | . + (" " * (8 - length)))ms " +
      "p95=\(.hook_latency_ms.p95 | tostring | . + (" " * (8 - length)))ms " +
      "p99=\(.hook_latency_ms.p99 | tostring | . + (" " * (8 - length)))ms | " +
      "card p50=\(.card_bytes.p50) p95=\(.card_bytes.p95) p99=\(.card_bytes.p99)"
    ' "$TMPDIR_AGG/$label-tagged.json"
  )
done

if [[ "${#PER_REPO_JSON[@]}" -eq 0 ]]; then
  echo "[hostile-bench-repos] no successful runs — aborting summary write" >&2
  exit 1
fi

# ---- aggregate JSON --------------------------------------------------------
jq -s --arg ts "$TS_UTC" \
  '{timestamp:$ts, runs:.}' \
  "${PER_REPO_JSON[@]}" > "$AGGREGATE"
echo
echo "[hostile-bench-repos] wrote $AGGREGATE"

# ---- append summary to hostile-baseline.txt -------------------------------
{
  echo ""
  for line in "${SUMMARY_LINES[@]}"; do
    echo "$line"
  done
} >> "$BASELINE"
echo "[hostile-bench-repos] appended summary to $BASELINE"
