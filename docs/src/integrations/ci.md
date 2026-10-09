# CI and GitHub Actions

## GitHub Action

```yaml
# .github/workflows/sqlsift.yml
name: SQL Lint
on: [push, pull_request]
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: yukikotani231/sqlsift@main  # or pin a release tag
        with:
          schema: db/schema.sql           # or schema-dir: db/migrations
          files: queries/**/*.sql
```

Errors are shown as annotations on the pull request diff. All inputs are optional when you have a `sqlsift.toml`:

| Input | Description |
|-------|-------------|
| `files` | Query files (space-separated paths or globs) |
| `schema` / `schema-dir` | Schema files, or a directory of migrations |
| `dialect` | `postgresql`, `mysql` or `sqlite` |
| `config` | Path to `sqlsift.toml` |
| `disable` | Rules to disable, e.g. `E0006 E0008` |
| `sarif-file` | Also write a SARIF report (see below) |
| `fail-on-error` | Fail the step on errors (default `true`) |
| `version` | `sqlsift-cli` version from npm (default `latest`) |
| `cli-path` | Use an existing `sqlsift` binary instead of installing from npm |

The `exit-code` output is `0` (clean), `1` (errors found) or `2` (configuration error).

## Any CI, with plain commands

```bash
npx sqlsift-cli check --schema schema.sql 'queries/**/*.sql'
```

The exit code is non-zero when errors are found. Inside a GitHub Actions job that runs sqlsift itself (a `make lint` step, a script, a container), add `--format github` to get annotations on the pull request diff.

## Rolling out a rule gradually

Make the rule a warning and cap the number of warnings with `--max-warnings` (or `max_warnings` in `sqlsift.toml`), so the backlog can only shrink:

```bash
sqlsift check -W missing-required-column --max-warnings 12
```

## Adopting sqlsift on an existing codebase

When a project already has many diagnostics, record them in a baseline and fail CI only on new ones:

```bash
sqlsift check --write-baseline          # writes sqlsift-baseline.json; commit it
```

```toml
# sqlsift.toml
baseline = "sqlsift-baseline.json"
```

From then on `sqlsift check` (and the editor) hides the recorded diagnostics, and new ones fail the check as usual. When a recorded problem is fixed, sqlsift prints a note; re-run `--write-baseline` to shrink the file. See [Baseline](../guide/suppression.md#an-existing-backlog-baseline).

## Re-check everything when the schema changes

sqlsift's main job is catching queries broken by a schema or migration change, and those query files usually aren't in the PR diff. The simplest setup is to always check every query file, as above: sqlsift checks hundreds of files in well under a second. If you only check changed files, check all of them whenever the schema changes:

```yaml
on: pull_request
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - id: changed
        env:
          BASE: ${{ github.event.pull_request.base.sha }}
        run: |
          changed=$(git diff --name-only --diff-filter=d "$BASE" HEAD)
          if grep -q '^db/' <<< "$changed"; then
            files='queries/**/*.sql'  # schema changed: check every query
          else
            files=$(grep '^queries/.*\.sql$' <<< "$changed" | tr '\n' ' ' || true)
          fi
          echo "files=$files" >> "$GITHUB_OUTPUT"
      - if: steps.changed.outputs.files != ''
        uses: yukikotani231/sqlsift@main
        with:
          schema-dir: db/migrations
          files: ${{ steps.changed.outputs.files }}
```

## GitHub Code Scanning (SARIF)

Show results in the Security tab and as code scanning alerts:

```yaml
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    permissions:
      security-events: write
    steps:
      - uses: actions/checkout@v4
      - uses: yukikotani231/sqlsift@main
        with:
          schema: db/schema.sql
          files: queries/**/*.sql
          sarif-file: results.sarif
          fail-on-error: "false"
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: results.sarif
```
