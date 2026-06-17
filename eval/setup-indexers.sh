#!/usr/bin/env bash
# Install the five SCIP indexers eval/run.sh expects:
#   scip, scip-go, scip-typescript, scip-python, rust-analyzer, scip-clang
#
# Idempotent — re-runs on a fully set-up box print "all 5 already installed"
# and exit 0. Never sudos; everything lands in $HOME (~/.local/bin or the
# user's existing toolchain prefix).
#
# Assumes Linux x86_64. Other arches print a clear error and bail per indexer.
set -euo pipefail

EVAL_DIR="$(cd "$(dirname "$0")" && pwd)"
LOCAL_BIN="${HOME}/.local/bin"
mkdir -p "$LOCAL_BIN"

# Pinned SCIP indexer versions. Eval ground truth in RESULTS.md is reproducible
# against these specific versions; bumping any of them requires re-running eval
# and updating RESULTS.md in the same commit. See eval/README.md "Reproducibility".
SCIP_VERSION="v0.8.1"               # sourcegraph/scip CLI
SCIP_CLANG_VERSION="v0.3.2"         # sourcegraph/scip-clang
SCIP_GO_VERSION="v0.2.7"            # sourcegraph/scip-go (go install @vX.Y.Z)
SCIP_TS_VERSION="0.4.0"             # @sourcegraph/scip-typescript (npm @X.Y.Z)
SCIP_PYTHON_VERSION="0.6.6"         # @sourcegraph/scip-python (npm @X.Y.Z)
# rust-analyzer: pinned via rustup toolchain — leave as rustup component.

ARCH="$(uname -m)"
OS="$(uname -s)"

# Tools we will report on at the end. Marked installed=1 once verified.
INDEXERS=(scip scip-go scip-typescript scip-python rust-analyzer scip-clang)
declare -A STATUS  # indexer -> "ok" | "missing" | "skipped:<reason>"

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

note() { printf '%s\n' "--- $*" >&2; }
warn() { printf '%s\n' "!!! $*" >&2; }

# ── Discover indexers across the layouts users commonly have ──
# npm under fnm/nvm installs to <node-prefix>/bin, not ~/.npm-global. We add
# every plausible bin dir to PATH for the lookup phase so an already-installed
# indexer isn't reported missing just because the user's interactive shell
# hasn't sourced the right line yet.
augment_path() {
  local extra="$1"
  [ -d "$extra" ] || return 0
  case ":$PATH:" in
    *":$extra:"*) ;;
    *) export PATH="$extra:$PATH" ;;
  esac
}

augment_path "$LOCAL_BIN"
augment_path "${HOME}/.cargo/bin"
augment_path "${HOME}/go/bin"
augment_path "${HOME}/.npm-global/bin"

# fnm: ~/.local/share/fnm/node-versions/<v>/installation/bin
if [ -d "${HOME}/.local/share/fnm/node-versions" ]; then
  for d in "${HOME}/.local/share/fnm/node-versions"/*/installation/bin; do
    augment_path "$d"
  done
fi
# nvm: ~/.nvm/versions/node/<v>/bin
if [ -d "${HOME}/.nvm/versions/node" ]; then
  for d in "${HOME}/.nvm/versions/node"/*/bin; do
    augment_path "$d"
  done
fi
# gvm: ~/.gvm/pkgsets/<go>/global/bin
if [ -d "${HOME}/.gvm/pkgsets" ]; then
  for d in "${HOME}/.gvm/pkgsets"/*/global/bin; do
    augment_path "$d"
  done
fi
# npm's own configured prefix, if any
if command -v npm >/dev/null 2>&1; then
  npm_prefix="$(npm prefix -g 2>/dev/null || true)"
  augment_path "${npm_prefix}/bin"
fi

# Distinguish Sourcegraph's `scip` (the SCIP Code Intelligence CLI) from
# unrelated binaries that happen to share the name — most notably ZIB's
# SCIP constraint solver, which ships in some Debian-flavoured repos as
# /usr/bin/scip. Sourcegraph's `scip --version` prints "scip version vX.Y.Z";
# anything else is the wrong tool.
is_real_scip() {
  command -v scip >/dev/null 2>&1 || return 1
  scip --version 2>/dev/null | grep -q '^scip version v'
}

# scip-clang's --version sanity check: it should print a semver-ish line.
is_real_scip_clang() {
  command -v scip-clang >/dev/null 2>&1 || return 1
  scip-clang --version >/dev/null 2>&1
}

# npm under the system node (default prefix /usr/local) will EACCES on a
# user-shell `npm install -g`. Funnel global installs into ~/.npm-global so
# we never need sudo. fnm/nvm prefixes are already user-writable; leave them.
maybe_redirect_npm_prefix() {
  command -v npm >/dev/null 2>&1 || return 0
  local prefix
  prefix="$(npm prefix -g 2>/dev/null || true)"
  case "$prefix" in
    /usr|/usr/local|/usr/local/lib/node_modules|"")
      export NPM_CONFIG_PREFIX="${HOME}/.npm-global"
      mkdir -p "$NPM_CONFIG_PREFIX/bin"
      augment_path "$NPM_CONFIG_PREFIX/bin"
      note "redirecting npm -g installs to $NPM_CONFIG_PREFIX (system prefix needs sudo)"
      ;;
  esac
}

# ── Per-indexer install functions ──

install_scip() {
  if [ "$OS" != "Linux" ] || [ "$ARCH" != "x86_64" ]; then
    warn "scip auto-install only wired for Linux x86_64 (got $OS/$ARCH). See https://github.com/scip-code/scip/releases"
    STATUS[scip]="skipped:unsupported-arch"
    return 0
  fi
  local url="https://github.com/scip-code/scip/releases/download/${SCIP_VERSION}/scip-linux-amd64.tar.gz"
  note "downloading scip ${SCIP_VERSION}"
  if ! curl -fsSL "$url" -o "$WORK/scip.tar.gz"; then
    warn "scip: download failed"
    STATUS[scip]="missing"
    return 0
  fi
  tar -xzf "$WORK/scip.tar.gz" -C "$WORK"
  install -m 0755 "$WORK/scip" "$LOCAL_BIN/scip"
  STATUS[scip]="ok"
}

install_scip_go() {
  if ! command -v go >/dev/null 2>&1; then
    warn "scip-go needs go on PATH (https://go.dev/dl/) — skipping"
    STATUS[scip-go]="skipped:no-go"
    return 0
  fi
  note "go install scip-go ${SCIP_GO_VERSION}"
  if go install "github.com/sourcegraph/scip-go/cmd/scip-go@${SCIP_GO_VERSION}"; then
    STATUS[scip-go]="ok"
  else
    warn "scip-go: go install failed"
    STATUS[scip-go]="missing"
  fi
}

install_scip_ts() {
  if ! command -v npm >/dev/null 2>&1; then
    warn "scip-typescript needs npm (node ≥18) — skipping"
    STATUS[scip-typescript]="skipped:no-npm"
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    local ver
    ver=$(node --version 2>/dev/null | sed 's/^v//;s/\..*//')
    if [ -n "$ver" ] && [ "$ver" -lt 18 ]; then
      warn "scip-typescript needs node ≥18 (have $ver) — skipping"
      STATUS[scip-typescript]="skipped:old-node"
      return 0
    fi
  fi
  maybe_redirect_npm_prefix
  note "npm install -g @sourcegraph/scip-typescript@${SCIP_TS_VERSION}"
  if npm install -g "@sourcegraph/scip-typescript@${SCIP_TS_VERSION}" >&2; then
    STATUS[scip-typescript]="ok"
  else
    warn "scip-typescript: npm install failed"
    STATUS[scip-typescript]="missing"
  fi
}

install_scip_python() {
  if ! command -v npm >/dev/null 2>&1; then
    warn "scip-python needs npm (node ≥18) — skipping"
    STATUS[scip-python]="skipped:no-npm"
    return 0
  fi
  maybe_redirect_npm_prefix
  note "npm install -g @sourcegraph/scip-python@${SCIP_PYTHON_VERSION}"
  if npm install -g "@sourcegraph/scip-python@${SCIP_PYTHON_VERSION}" >&2; then
    STATUS[scip-python]="ok"
  else
    warn "scip-python: npm install failed"
    STATUS[scip-python]="missing"
  fi
}

install_rust_analyzer() {
  if command -v rustup >/dev/null 2>&1; then
    note "rustup component add rust-analyzer"
    if rustup component add rust-analyzer >&2; then
      STATUS[rust-analyzer]="ok"
      return 0
    fi
    warn "rust-analyzer: rustup add failed"
  else
    warn "rust-analyzer needs rustup (https://rustup.rs) — skipping"
  fi
  STATUS[rust-analyzer]="skipped:no-rustup"
}

install_scip_clang() {
  if [ "$OS" != "Linux" ] || [ "$ARCH" != "x86_64" ]; then
    warn "scip-clang auto-install only wired for Linux x86_64 (got $OS/$ARCH). See https://github.com/sourcegraph/scip-clang/releases"
    STATUS[scip-clang]="skipped:unsupported-arch"
    return 0
  fi
  local url="https://github.com/sourcegraph/scip-clang/releases/download/${SCIP_CLANG_VERSION}/scip-clang-x86_64-linux"
  note "downloading scip-clang ${SCIP_CLANG_VERSION}"
  if curl -fsSL "$url" -o "$LOCAL_BIN/scip-clang"; then
    chmod +x "$LOCAL_BIN/scip-clang"
    STATUS[scip-clang]="ok"
  else
    warn "scip-clang: download failed"
    STATUS[scip-clang]="missing"
  fi
}

# ── Main loop: check, then install only if missing ──
need_install=0
check_one() {
  local indexer="$1"
  case "$indexer" in
    scip)        is_real_scip ;;
    scip-clang)  is_real_scip_clang ;;
    *)           command -v "$indexer" >/dev/null 2>&1 ;;
  esac
}
for indexer in "${INDEXERS[@]}"; do
  if check_one "$indexer"; then
    STATUS[$indexer]="ok"
  else
    STATUS[$indexer]="missing"
    need_install=1
  fi
done

if [ "$need_install" -eq 0 ]; then
  echo "all 5 already installed; nothing to do" >&2
else
  for indexer in "${INDEXERS[@]}"; do
    [ "${STATUS[$indexer]}" = "ok" ] && continue
    case "$indexer" in
      scip)            install_scip ;;
      scip-go)         install_scip_go ;;
      scip-typescript) install_scip_ts ;;
      scip-python)     install_scip_python ;;
      rust-analyzer)   install_rust_analyzer ;;
      scip-clang)      install_scip_clang ;;
    esac
  done

  # Re-augment PATH after installs (rustup adds to ~/.cargo/bin, npm may add
  # to its prefix bin, etc.) and re-verify what actually landed.
  augment_path "$LOCAL_BIN"
  augment_path "${HOME}/.cargo/bin"
  augment_path "${HOME}/go/bin"
  if command -v npm >/dev/null 2>&1; then
    npm_prefix="$(npm prefix -g 2>/dev/null || true)"
    augment_path "${npm_prefix}/bin"
  fi
  for indexer in "${INDEXERS[@]}"; do
    if check_one "$indexer"; then
      STATUS[$indexer]="ok"
    fi
  done
fi

# ── Summary ──
echo >&2
echo "indexer status:" >&2
for indexer in "${INDEXERS[@]}"; do
  loc="$(command -v "$indexer" 2>/dev/null || echo '(not on PATH)')"
  printf '  %-16s %-7s %s\n' "$indexer" "${STATUS[$indexer]}" "$loc" >&2
done

# Corpus availability — mirror run.sh's REPOS list (kept in sync by hand).
echo >&2
echo "corpus availability:" >&2
declare -A CORPUS_INDEXER=(
  [gin]=scip-go
  [hono]=scip-typescript
  [click]=scip-python
  [ripgrep]=rust-analyzer
  [abyss]=rust-analyzer
  [cmark]=scip-clang
)
ready=0
total=0
for name in gin hono click ripgrep abyss cmark; do
  total=$((total + 1))
  ind="${CORPUS_INDEXER[$name]}"
  if [ "${STATUS[$ind]}" = "ok" ]; then
    printf '  %-8s ready (%s)\n' "$name" "$ind" >&2
    ready=$((ready + 1))
  else
    printf '  %-8s SKIP   (%s: %s)\n' "$name" "$ind" "${STATUS[$ind]}" >&2
  fi
done
printf '%d/%d corpora can run; scip itself %s\n' "$ready" "$total" "${STATUS[scip]}" >&2

# PATH hint — if anything we installed isn't visible to a vanilla login shell,
# tell the user how to fix it.
hint=0
for indexer in "${INDEXERS[@]}"; do
  [ "${STATUS[$indexer]}" = "ok" ] || continue
  loc="$(command -v "$indexer" 2>/dev/null || true)"
  dir="$(dirname "$loc")"
  case ":$PATH:" in
    *":$dir:"*) ;;
    *) hint=1; warn "$indexer at $loc — add $dir to PATH";;
  esac
done
[ "$hint" -eq 0 ] || warn "add the lines above to ~/.bashrc / ~/.zshrc so eval/run.sh sees them"

exit 0
