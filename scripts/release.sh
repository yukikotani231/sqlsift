#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh 0.1.0-alpha.4
#
# This script automates the release process:
# 1. Updates version in Cargo.toml (workspace + dependency)
# 2. Updates Cargo.lock
# 3. Adds a new section to CHANGELOG.md
# 4. Runs tests, clippy, and fmt checks
# 5. Creates a release branch, commits, and pushes
# 6. Creates a PR via gh CLI
#
# After the PR is merged, everything is fully automated:
#   auto-tag.yml: creates git tag + triggers release workflow via workflow_dispatch
#   release.yml: builds binaries + creates GitHub Release + publishes npm package
#
# Manual tagging should not be needed. If the automation fails,
# you can use: ./scripts/release.sh --tag <version>

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# --- Tag mode ---
if [[ "${1:-}" == "--tag" ]]; then
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
    exit 0
fi

# --- Release prep mode ---
VERSION="${1:?Usage: $0 <version>}"
TAG="v${VERSION}"

# Validate version format
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+'; then
    echo "ERROR: Version must be in semver format (e.g., 0.1.0, 0.1.0-alpha.4)"
    exit 1
fi

# Check prerequisites
command -v gh >/dev/null 2>&1 || { echo "ERROR: gh CLI is required. Install: https://cli.github.com/"; exit 1; }

# Check for clean working tree
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "ERROR: Working tree is not clean. Commit or stash changes first."
    exit 1
fi

# Check we're on main
BRANCH=$(git branch --show-current)
if [[ "$BRANCH" != "main" ]]; then
    echo "ERROR: Must be on main branch (currently on: $BRANCH)"
    exit 1
fi

# Get current version
OLD_VERSION=$(grep -m1 'version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
echo "==> Updating version: ${OLD_VERSION} -> ${VERSION}"

# 1. Update version in Cargo.toml
sed -i '' "s/version = \"${OLD_VERSION}\"/version = \"${VERSION}\"/g" Cargo.toml
echo "    Updated Cargo.toml"

# 2. Update Cargo.lock
cargo check --quiet 2>/dev/null
echo "    Updated Cargo.lock"

# 3. Update CHANGELOG.md
TODAY=$(date +%Y-%m-%d)
CHANGELOG_ENTRY="## [${VERSION}] - ${TODAY}

### Added

### Fixed

### Changed
"

# Insert after [Unreleased] line
sed -i '' "/^## \[Unreleased\]$/a\\
\\
${CHANGELOG_ENTRY}" CHANGELOG.md

# Update links at bottom
sed -i '' "s|\[Unreleased\]: \(.*\)/compare/v${OLD_VERSION}\.\.\.HEAD|[Unreleased]: \1/compare/v${VERSION}...HEAD\n[${VERSION}]: \1/compare/v${OLD_VERSION}...v${VERSION}|" CHANGELOG.md

echo "    Updated CHANGELOG.md (please edit the entries before committing)"

# 4. Run checks
echo "==> Running checks..."
cargo test --quiet 2>&1
echo "    Tests passed"
cargo clippy --all-targets -- -D warnings 2>&1 | tail -1
echo "    Clippy passed"
cargo fmt --check 2>&1
echo "    Format OK"

# 5. Open CHANGELOG for editing
echo ""
echo "==> CHANGELOG.md has been prepared with empty sections."
echo "    Please edit it now to add the release notes."
echo ""
read -p "Press Enter when CHANGELOG.md is ready (or Ctrl+C to abort)..."

# 6. Create release branch, commit, push, and PR
RELEASE_BRANCH="release/v${VERSION}"
echo "==> Creating branch ${RELEASE_BRANCH}..."
git checkout -b "$RELEASE_BRANCH"

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "Prepare v${VERSION} release"

echo "==> Pushing branch..."
git push -u origin "$RELEASE_BRANCH" 2>&1

echo "==> Creating PR..."
PR_URL=$(gh pr create \
    --title "Release v${VERSION}" \
    --body "## Summary
- Version bump to ${VERSION}
- See CHANGELOG.md for details

Merging this PR will automatically create the \`v${VERSION}\` tag and trigger the release workflow.")

echo ""
echo "================================================"
echo "  Release PR created: ${PR_URL}"
echo "================================================"
echo ""
echo "Next steps:"
echo "  1. Review and merge the PR"
echo "  2. Tag is created automatically -> release workflow runs"
echo ""
