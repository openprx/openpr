#!/usr/bin/env bash
# Usage: ./scripts/bump-version.sh [major|minor|patch]
# Default: patch
set -euo pipefail

TYPE="${1:-patch}"
CARGO="Cargo.toml"
PKG="frontend/package.json"

# Read current version from Cargo.toml
CURRENT=$(grep '^version = ' "$CARGO" | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

case "$TYPE" in
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  patch) PATCH=$((PATCH + 1)) ;;
  *) echo "Usage: $0 [major|minor|patch]"; exit 1 ;;
esac

NEW="${MAJOR}.${MINOR}.${PATCH}"

# Update Cargo.toml
sed -i "0,/^version = \".*\"/s//version = \"${NEW}\"/" "$CARGO"

# Update frontend/package.json
if [ -f "$PKG" ]; then
  sed -i "s/\"version\": \".*\"/\"version\": \"${NEW}\"/" "$PKG"
fi

echo "${CURRENT} → ${NEW}"
