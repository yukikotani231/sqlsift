# Pre-commit hooks

sqlsift is fast enough to run on every commit.

## Plain Git hook

Check the **staged** version of each changed query file, not the working copy, by piping it through stdin:

```bash
#!/usr/bin/env bash
# .git/hooks/pre-commit (or .githooks/pre-commit with `git config core.hooksPath .githooks`)
set -euo pipefail

status=0
while IFS= read -r file; do
  git show ":$file" | sqlsift check --stdin-filename "$file" - || status=1
done < <(git diff --cached --name-only --diff-filter=ACM -- '*.sql' '*.ts' '*.tsx' '*.js' '*.jsx' '*.mts' '*.cts' '*.mjs' '*.cjs' '*.vue' '*.svelte')
exit $status
```

The pathspec covers `.sql` files and the files sqlsift checks for [SQL in tagged template literals](../guide/queries.md#sql-in-typescript-and-javascript) (TypeScript, JavaScript, Vue and Svelte), so a commit that only changes embedded SQL is still checked. If your project has no embedded SQL, keep just `'*.sql'`, otherwise the hook runs on every commit that touches those files for nothing.

This uses the schema settings from `sqlsift.toml`. When a schema or migration file is staged, consider checking every query (`sqlsift check`) instead, since any of them may be affected.

## pre-commit framework

With [pre-commit](https://pre-commit.com/), a local hook that checks all query files works well, because a run is fast:

```yaml
# .pre-commit-config.yaml
repos:
  - repo: local
    hooks:
      - id: sqlsift
        name: sqlsift
        entry: npx --yes sqlsift-cli check
        language: system
        files: \.(sql|ts|tsx|js|jsx|mts|cts|mjs|cjs|vue|svelte)$
        pass_filenames: false
```

`pass_filenames: false` makes sqlsift check the `files` from `sqlsift.toml`, so a schema change re-checks every query.

As with the plain hook, `files` also matches the extensions of files with [embedded SQL](../guide/queries.md#sql-in-typescript-and-javascript), so the hook runs when only those change. Drop them if your project has none, and make sure the `files` in `sqlsift.toml` include them (for example `src/**/*.ts`) so they are actually checked.

## lefthook / husky

Any hook manager works the same way: run `sqlsift check` (or `npx sqlsift-cli check`) and let it read `sqlsift.toml`.
