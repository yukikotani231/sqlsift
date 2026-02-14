#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh --tag <version>
#
# Manual fallback for tagging a release.
# Normally, releases are handled automatically by release-please:
#   1. Conventional Commits on main trigger release-please to create a Release PR
#   2. Merging the Release PR creates a git tag and GitHub Release
#   3. The tag triggers cargo-dist (release.yml) to build and publish binaries
#
# Use this script only if the automation fails and you need to manually create a tag.

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ "${1:-}" != "--tag" ]]; then
    echo "Usage: $0 --tag <version>"
    echo ""
    echo "This script is a manual fallback for creating release tags."
    echo "Normally, releases are handled by release-please (see .github/workflows/release-please.yml)."
    echo ""
    echo "Example: $0 --tag 0.1.0-alpha.9"
    exit 1
fi

VERSION="${2:?Usage: $0 --tag <version>}"
TAG="v${VERSION}"

echo "==> Switching to main and pulling latest..."
git checkout main
git pull origin main

# Verify the version matches
CURRENT_VERSION=$(grep -m1 'version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
if [[ "$CURRENT_VERSION" != "$VERSION" ]]; then
    echo "ERROR: Cargo.toml version ($CURRENT_VERSION) does not match requested version ($VERSION)"
    exit 1
fi

echo "==> Creating tag ${TAG}..."
git tag "$TAG"
git push origin "$TAG"
echo ""
echo "Tag ${TAG} pushed. The release workflow will build and publish automatically."
echo "Monitor progress: gh run list --limit 3"
