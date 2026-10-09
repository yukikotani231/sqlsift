# Suppressing diagnostics

Sometimes a query is right and sqlsift is wrong, or a legacy file isn't worth fixing yet. There are three levels of suppression, from narrowest to widest.

> If sqlsift reports valid SQL, please also [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with a minimal schema and query so it can be fixed.

## One line: `sqlsift:disable`

A `-- sqlsift:disable` comment on its own line applies to the next line; at the end of a line, it applies to that line.

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
```

## One file: `sqlsift:disable-file`

```sql
-- sqlsift:disable-file E0006, missing-required-column

-- or turn off every rule for this file, including parse errors (E1000)
-- sqlsift:disable-file
```

A `disable-file` comment may appear anywhere in the file (conventionally at the top) and applies to every line, before and after it. It is separate from `sqlsift:disable`: it never acts as a next-line directive, and `sqlsift:disable` never disables a rule for the whole file.

## Whole files, without editing them: `ignore`

To skip generated or archived files entirely, list them in `ignore` in `sqlsift.toml`, or pass `--ignore`:

```toml
ignore = ["queries/archive/**", "**/*.generated.sql"]
```

Ignored files aren't parsed at all, and editors show no diagnostics for them. See [Checking queries](queries.md#choosing-files) for the pattern rules.

## Project-wide: rule levels

To turn a rule off everywhere, set its level instead; see [Rules and levels](rules.md).
