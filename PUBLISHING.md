# Publishing Guide

This document describes how to publish sqlsurge to crates.io and npm.

## Prerequisites

### For crates.io
1. Create an account on [crates.io](https://crates.io/)
2. Get an API token: `cargo login`
3. Ensure you're a member/owner of the crates

### For npm
1. Create an account on [npmjs.com](https://www.npmjs.com/)
2. Login: `npm login`
3. (Optional) Create an organization for scoped packages

## Publishing to crates.io

### Pre-publish Checklist
- [ ] All tests pass: `cargo test --workspace`
- [ ] Clippy is clean: `cargo clippy --all-targets -- -D warnings`
- [ ] Format is correct: `cargo fmt --check`
- [ ] Documentation builds: `cargo doc --no-deps`
- [ ] Version updated in `Cargo.toml` (workspace.package.version)
- [ ] CHANGELOG.md updated with new version
- [ ] README.md is up to date
- [ ] All changes committed to git
- [ ] Git tag created: `git tag v0.1.0`

### Publishing Steps

1. **Dry run (verify package contents)**
   ```bash
   cargo package --list
   ```

2. **Test the package builds**
   ```bash
   cargo package
   ```

3. **Publish sqlsurge-core first** (dependency must be published first)
   ```bash
   cd crates/sqlsurge-core
   cargo publish
   cd ../..
   ```

4. **Wait for core to be available** (usually takes a few minutes)
   ```bash
   # Check if it's available
   cargo search sqlsurge-core
   ```

5. **Publish sqlsurge-cli**
   ```bash
   cd crates/sqlsurge-cli
   cargo publish
   cd ../..
   ```

6. **Push tags to GitHub**
   ```bash
   git push origin main
   git push origin v0.1.0
   ```

7. **Create GitHub Release**
   - Go to https://github.com/yukikotani231/sqlsurge/releases/new
   - Select the tag you just pushed
   - Copy the CHANGELOG entry for this version
   - Publish release

## Publishing to npm

npm publishing is handled automatically by cargo-dist.
When a version tag (e.g., `v0.1.0`) is pushed, the GitHub Actions release workflow
builds platform-specific binaries and publishes the npm package (`sqlsurge-cli`).

**Prerequisite:** Set `NPM_TOKEN` in GitHub repository secrets.

## Post-publish

1. **Verify installation works**
   ```bash
   cargo install sqlsurge-cli
   sqlsurge --version
   ```

2. **Test in a fresh project**
   ```bash
   mkdir test-sqlsurge && cd test-sqlsurge
   cargo init
   cargo add sqlsurge-core
   # Write a simple test
   cargo test
   ```

3. **Announce**
   - Twitter/X
   - Reddit (r/rust)
   - This Week in Rust
   - Rust Users Forum

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
