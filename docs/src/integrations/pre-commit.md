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
done < <(git diff --cached --name-only --diff-filter=ACM -- '*.sql')
exit $status
```

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
        files: \.sql$
        pass_filenames: false
```

`pass_filenames: false` makes sqlsift check the `files` from `sqlsift.toml`, so a schema change re-checks every query.

## lefthook / husky

Any hook manager works the same way: run `sqlsift check` (or `npx sqlsift-cli check`) and let it read `sqlsift.toml`.
