#!/usr/bin/env bash
# abyss installer: prebuilt binary from GitHub Releases, source build fallback.
#
#   curl -fsSL https://raw.githubusercontent.com/telagod/abyss/main/install.sh | bash
#   ./install.sh --from-source          # force cargo build
#   ABYSS_VERSION=v0.3.0 ./install.sh   # pin a version (default: latest)
set -euo pipefail

REPO="telagod/abyss"
INSTALL_DIR="${ABYSS_INSTALL_DIR:-${HOME}/.local/bin}"
VERSION="${ABYSS_VERSION:-latest}"
MODE="${1:-auto}"

say() { echo "[abyss-install] $*" >&2; }

install_from_source() {
  if ! command -v cargo &>/dev/null; then
    say "Rust not found. Install: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
  fi
  local repo_dir
  if [ -f "$(dirname "$0")/Cargo.toml" ]; then
    repo_dir="$(cd "$(dirname "$0")" && pwd)"
  else
    repo_dir="$(mktemp -d)/abyss"
    say "cloning ${REPO}..."
    git clone -q --depth 1 "https://github.com/${REPO}.git" "$repo_dir"
  fi
  say "building (release, slim)..."
  (cd "$repo_dir" && cargo build --release --quiet)
  mkdir -p "$INSTALL_DIR"
  cp "$repo_dir/target/release/abyss" "$INSTALL_DIR/abyss"
  chmod +x "$INSTALL_DIR/abyss"
}

install_prebuilt() {
  local os arch target url tmp
  case "$(uname -s)" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64)  arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) return 1 ;;
  esac
  target="${arch}-${os}"

  if [ "$VERSION" = "latest" ]; then
    url="https://github.com/${REPO}/releases/latest/download/abyss-${target}.tar.gz"
  else
    url="https://github.com/${REPO}/releases/download/${VERSION}/abyss-${target}.tar.gz"
  fi

  tmp="$(mktemp -d)"
  say "downloading ${url}..."
  if ! curl -fsSL "$url" -o "$tmp/abyss.tar.gz"; then
    say "prebuilt binary unavailable for ${target} (no release yet, or repo not public)"
    return 1
  fi
  tar -xzf "$tmp/abyss.tar.gz" -C "$tmp"
  mkdir -p "$INSTALL_DIR"
  cp "$tmp/abyss" "$INSTALL_DIR/abyss"
  chmod +x "$INSTALL_DIR/abyss"
  rm -rf "$tmp"
}

echo "=== abyss installer ===" >&2

if [ "$MODE" = "--from-source" ]; then
  install_from_source
else
  install_prebuilt || {
    say "falling back to source build"
    install_from_source
  }
fi

say "installed: $INSTALL_DIR/abyss ($("$INSTALL_DIR/abyss" --version 2>/dev/null || echo unknown))"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  say "NOTE: $INSTALL_DIR is not on your PATH — add: export PATH=\"$INSTALL_DIR:\$PATH\""
fi

cat >&2 <<'USAGE'

usage:
  abyss index          # build index (~seconds)
  abyss context FILE   # full pre-edit context for a file
  abyss callers X      # who calls X
  abyss impact X       # blast radius of changing X
  abyss map            # hotspots + coupling
  abyss --help         # all commands
USAGE
