#!/usr/bin/env bash
# abyss installer: build from source and install to ~/.local/bin
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
REPO_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== abyss installer ==="

# Check Rust
if ! command -v cargo &>/dev/null; then
  echo "Rust not found. Install: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  exit 1
fi

# Build
echo "building (release)..."
cd "$REPO_DIR"
cargo build --release --quiet

# Install
mkdir -p "$INSTALL_DIR"
cp target/release/abyss "$INSTALL_DIR/abyss"
chmod +x "$INSTALL_DIR/abyss"

echo "installed: $INSTALL_DIR/abyss"

# Check PATH
if ! echo "$PATH" | tr ':' '\n' | grep -q "$INSTALL_DIR"; then
  echo ""
  echo "⚠ $INSTALL_DIR is not in PATH. Add to your shell config:"
  echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi

echo ""
echo "usage:"
echo "  abyss index          # build index (~5s)"
echo "  abyss callers X      # who calls X"
echo "  abyss impact X       # blast radius of changing X"
echo "  abyss map            # hotspots + coupling"
echo "  abyss --help         # all commands"
