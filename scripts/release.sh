#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh <patch|minor|major> [--dry-run]
#
# Automates the release flow:
#   1. Bump version in Cargo.toml
#   2. Insert CHANGELOG heading (you fill in the body)
#   3. cargo build + cargo test (gate)
#   4. Commit "release: vX.Y.Z"
#   5. Tag vX.Y.Z
#   6. Push (triggers release.yml → binary artifacts)

BUMP="${1:?Usage: release.sh <patch|minor|major> [--dry-run]}"
DRY_RUN="${2:-}"

# Parse current version
CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$BUMP" in
  patch) PATCH=$((PATCH + 1)) ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  *) echo "Invalid bump: $BUMP (use patch|minor|major)"; exit 1 ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
TAG="v${NEW_VERSION}"
DATE=$(date +%Y-%m-%d)

echo "📦 Release: ${CURRENT} → ${NEW_VERSION} (${BUMP})"
echo "   Tag: ${TAG}"
echo "   Date: ${DATE}"

if [[ "$DRY_RUN" == "--dry-run" ]]; then
  echo "   (dry run — no changes)"
  exit 0
fi

# Check clean working tree
if [[ -n "$(git status --porcelain -- ':!.claude/')" ]]; then
  echo "❌ Working tree not clean. Commit or stash first."
  exit 1
fi

# 1. Bump Cargo.toml
sed -i "s/^version = \"${CURRENT}\"/version = \"${NEW_VERSION}\"/" Cargo.toml
echo "✓ Cargo.toml → ${NEW_VERSION}"

# 2. Cargo.lock update
cargo check --quiet 2>/dev/null || true
echo "✓ Cargo.lock updated"

# 3. Insert CHANGELOG heading
HEADER="## ${TAG} — ${DATE}"
if grep -q "^## ${TAG}" CHANGELOG.md 2>/dev/null; then
  echo "  CHANGELOG already has ${TAG} heading"
else
  # Insert after the first line (# Changelog)
  sed -i "/^# Changelog$/a\\
\\
${HEADER}\\
\\
_Fill in release notes here._" CHANGELOG.md
  echo "✓ CHANGELOG.md heading inserted (edit the body before committing)"
fi

# 4. Build + test gate
echo "🔨 Building..."
cargo build --quiet
echo "🧪 Testing..."
cargo test --quiet 2>&1 | tail -3
echo "✓ Build + test passed"

# 5. Commit + tag
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release: ${TAG}"
git tag -a "${TAG}" -m "Release ${TAG}"
echo "✓ Committed and tagged ${TAG}"

# 6. Push
echo ""
echo "Ready to push. Run:"
echo "  git push && git push --tags"
echo ""
echo "This triggers .github/workflows/release.yml → binary artifacts."
