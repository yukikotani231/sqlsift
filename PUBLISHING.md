# Publishing Guide

This document describes how to publish sqlsift releases.

## Automated Release (Recommended)

Releases are fully automated using [release-plz](https://release-plz.dev/):

1. **Write Conventional Commits** on the `main` branch:
   - `feat: add new feature` → bumps minor
   - `fix: fix a bug` → bumps patch
   - `feat!: breaking change` → bumps major

2. **release-plz creates a Release PR** automatically:
   - Updates version in `Cargo.toml` (workspace version)
   - Updates `CHANGELOG.md` with commit messages

3. **Merge the Release PR** to trigger the release:
   - release-plz creates a git tag (`v{version}`)
   - `release.yml` (cargo-dist) builds platform-specific binaries
   - npm package (`sqlsift-cli`) is published automatically

## Manual Fallback

If the automation fails, use the manual tag script:

```bash
./scripts/release.sh --tag 0.1.0-alpha.9
```

This creates a git tag and pushes it, triggering the cargo-dist release workflow.

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
