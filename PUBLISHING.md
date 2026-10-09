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
./scripts/release.sh --tag 0.1.2
```

This creates a git tag and pushes it, triggering the cargo-dist release workflow.

## Prerequisites

### For npm
- Configure [OIDC trusted publishing](https://docs.npmjs.com/generating-provenance-statements#publishing-packages-with-provenance-via-github-actions) on npmjs.com for the `sqlsift-cli` package
  - Repository owner: `yukikotani231`, Repository: `sqlsift`, Workflow: `release.yml`
- No `NPM_TOKEN` secret needed — authentication is handled via GitHub Actions OIDC

### For crates.io (manual, if needed)
1. Create an account on [crates.io](https://crates.io/)
2. Get an API token: `cargo login`
3. Publish core first, then CLI:
   ```bash
   cd crates/sqlsift-core && cargo publish && cd ../..
   # Wait a few minutes for index update
   cd crates/sqlsift-cli && cargo publish && cd ../..
   ```

### For VS Code Marketplace

The `publish-vscode.yml` workflow builds platform-specific VSIX files and uploads them to GitHub Releases automatically. To also publish to VS Code Marketplace:

1. Create a publisher on [Visual Studio Marketplace](https://marketplace.visualstudio.com/manage)
   - Publisher ID: `sqlsift`
2. The workflow signs in to the Marketplace with a Microsoft Entra ID app over GitHub OIDC
   (`vsce publish --azure-credential`), so no token is stored and nothing expires. Global
   Azure DevOps PATs (the old `VSCE_PAT`) are retired on 2026-12-01.
   1. In the [Azure portal](https://portal.azure.com/), Microsoft Entra ID > App registrations >
      New registration (single tenant). No Azure subscription is needed.
   2. In the app, Certificates & secrets > Federated credentials > Add credential:
      - Scenario: "GitHub Actions deploying Azure resources"
      - Organization: `yukikotani231`, Repository: `sqlsift`
      - Entity type: Environment, name: `vscode-marketplace`
   3. In GitHub, Settings > Environments > `vscode-marketplace`, add the variables
      `AZURE_CLIENT_ID` (the app's Application (client) ID) and `AZURE_TENANT_ID`
      (Directory (tenant) ID). They are identifiers, not secrets.
   4. Run "Publish VS Code Extension" manually with **identity** checked. The
      "Show Marketplace identity" step prints the `id` the Marketplace knows the app by
      (it can only be read while signed in as the app); "Verify publish rights" fails
      until the next step is done.
   5. On the [publisher page](https://marketplace.visualstudio.com/manage/publishers/sqlsift),
      Members > Add, enter that `id` and give it the **Contributor** role. Run identity
      mode again: "Verify publish rights" should pass.
3. Add repository variable `PUBLISH_VSCODE` = `true` in Settings > Secrets and variables > Actions > Variables

If the app signs in but publishing fails with `InvalidAccessException`, use a user-assigned
managed identity (needs an Azure subscription) instead of the app registration; the steps
are the same.

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
