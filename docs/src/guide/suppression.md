# Suppressing diagnostics

Sometimes a query is right and sqlsift is wrong, or a legacy file isn't worth fixing yet. There are three levels of suppression, from narrowest to widest.

> If sqlsift reports valid SQL, please also [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with a minimal schema and query so it can be fixed.

## One line: `sqlsift:disable`

A `-- sqlsift:disable` comment on its own line applies to the next line; at the end of a line, it applies to that line.
Whitespace around the colon is allowed, as in `-- sqlsift : disable` or `-- sqlsift: disable`.

```sql
-- Suppress a specific rule on the next line
-- sqlsift:disable E0002
SELECT legacy_col FROM users;

-- Suppress on the same line
SELECT legacy_col FROM users; -- sqlsift:disable E0002

-- Suppress several rules (codes or names)
SELECT bad_col FROM missing_table; -- sqlsift:disable E0001, column-not-found

-- Suppress all rules on the next line
-- sqlsift:disable
SELECT bad_col FROM missing_table;

-- Whitespace around the colon is also accepted
-- sqlsift : disable-file E0002
```

## One file: `sqlsift:disable-file`

```sql
-- sqlsift:disable-file E0006, missing-required-column

-- or turn off every rule for this file, including parse errors (E1000)
-- sqlsift:disable-file

-- Whitespace around the colon is also accepted
-- sqlsift : disable-file E0002
```

A `disable-file` comment may appear anywhere in the file (conventionally at the top) and applies to every line, before and after it. It is separate from `sqlsift:disable`: it never acts as a next-line directive, and `sqlsift:disable` never disables a rule for the whole file.

## Whole files, without editing them: `ignore`

To skip generated or archived files entirely, list them in `ignore` in `sqlsift.toml`, or pass `--ignore`:

```toml
ignore = ["queries/archive/**", "**/*.generated.sql"]
```

Ignored files aren't parsed at all, and editors show no diagnostics for them. See [Checking queries](queries.md#choosing-files) for the pattern rules.

## An existing backlog: baseline

To adopt sqlsift on a project that already has many diagnostics, record them in a baseline file and get reports only for new ones:

```bash
sqlsift check --write-baseline
```

This writes every current diagnostic (errors and warnings) to `sqlsift-baseline.json` and exits `0`. Commit the file and point sqlsift at it, in `sqlsift.toml` (so the language server uses it too) or with `--baseline <PATH>`:

```toml
baseline = "sqlsift-baseline.json"
```

Diagnostics in the baseline are not printed and don't count toward the exit code, `--max-errors` or `--max-warnings`. `sqlsift check` prints how many were hidden, and a note when baseline entries no longer occur (the problem was fixed); re-run `sqlsift check --write-baseline` to remove them. That note never fails the check.

A diagnostic matches a baseline entry by its file (relative to the baseline file), rule code and the text of the statement it is in, with comments and whitespace ignored. Adding lines or statements elsewhere in the file, or reformatting the statement, keeps the match; changing the statement itself makes its diagnostics new again. The same mistake made twice in one statement is matched by its position among them. Each entry also stores the line and message, for people reading the file.

Write the baseline over the same files that CI checks (by default the `files` in `sqlsift.toml`): `--write-baseline` replaces the whole file with the diagnostics of the files given.

## Project-wide: rule levels

To turn a rule off everywhere, set its level instead; see [Rules and levels](rules.md).
