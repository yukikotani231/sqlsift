# Publishing Guide

This document describes how to publish sqlsift releases.

## Quick Release (Recommended)

Use the release script for a streamlined process:

```bash
# 1. Prepare the release (updates version, CHANGELOG, creates PR)
./scripts/release.sh 0.2.0

# 2. Edit CHANGELOG.md when prompted, then press Enter

# 3. Review and merge the PR on GitHub
#    -> Tag is created automatically
#    -> Release workflow builds and publishes automatically
```

The script handles:
- Version bump in `Cargo.toml`
- `Cargo.lock` update
- `CHANGELOG.md` scaffolding
- Running tests, clippy, and fmt checks
- Creating a release branch, commit, and PR

After the PR is merged:
1. `auto-tag.yml` workflow detects the merged `release/v*` branch and creates the git tag
2. The tag triggers `release.yml` (cargo-dist) which automatically:
   - Builds platform-specific binaries (macOS, Linux, Windows)
   - Creates a GitHub Release with artifacts
   - Publishes the npm package (`sqlsift-cli`)

## Prerequisites

### For npm
- Set `NPM_TOKEN` in GitHub repository secrets
- npm publishing is handled automatically by cargo-dist

### For crates.io (manual, if needed)
1. Create an account on [crates.io](https://crates.io/)
2. Get an API token: `cargo login`
3. Publish core first, then CLI:
   ```bash
   cd crates/sqlsift-core && cargo publish && cd ../..
   # Wait a few minutes for index update
   cd crates/sqlsift-cli && cargo publish && cd ../..
   ```

## Post-publish

1. **Verify installation works**
   ```bash
   cargo install sqlsift-cli
   sqlsift --version
   ```

2. **Test in a fresh project**
   ```bash
   mkdir test-sqlsift && cd test-sqlsift
   cargo init
   cargo add sqlsift-core
   cargo test
   ```

## Version Strategy

- Follow [Semantic Versioning](https://semver.org/)
- 0.y.z: Initial development (breaking changes allowed)
- 1.0.0: First stable release
- Patch (0.1.x): Bug fixes
- Minor (0.x.0): New features (backward compatible)
- Major (x.0.0): Breaking changes

## Troubleshooting

### "crate not found" error when publishing CLI
- Wait a few minutes for crates.io index to update after publishing core
- Try `cargo update` to refresh the index

### Permission denied
- Ensure you're logged in: `cargo login`
- Check you're an owner: Visit crate page on crates.io

### README not found
- Ensure `readme = "../../README.md"` path is correct
- Check the file exists: `ls README.md`
