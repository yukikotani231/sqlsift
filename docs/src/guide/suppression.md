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

In TypeScript, JavaScript, Vue and Svelte files, write the directives as code comments (`// sqlsift:disable-file`, `/* sqlsift:disable E0002 */`) or as SQL comments inside a template; a `disable-file` comment inside one template applies to the whole file (see [SQL in TypeScript and JavaScript](queries.md#directives)). In Rust files, write them as Rust comments (`// sqlsift:disable E0002` on the line before a query), in Python files as Python comments (`# sqlsift:disable E0002`), and in Go files as Go comments (`// sqlsift:disable E0002`).

## Whole files, without editing them: `ignore`

To skip generated or archived files entirely, list them in `ignore` in `sqlsift.toml`, or pass `--ignore`:

```toml
ignore = ["queries/archive/**", "**/*.generated.sql"]
```

Ignored files aren't parsed at all, and editors show no diagnostics for them. See [Checking queries](queries.md#choosing-files) for the pattern rules.

## An existing backlog: baseline

To adopt sqlsift on a project that already has many diagnostics, record them in a baseline file and get reports only for new ones. (In GitHub Actions, the action's [`diff-base` input](../integrations/ci.md#report-only-what-a-pull-request-introduces) does this for each pull request without a committed file.)

```bash
sqlsift check --write-baseline
```

This writes every current diagnostic (errors and warnings) to `sqlsift-baseline.json` and exits `0`. Commit the file and point sqlsift at it, in `sqlsift.toml` (so the language server uses it too) or with `--baseline <PATH>`:

```toml
baseline = "sqlsift-baseline.json"
```

Diagnostics in the baseline are not printed and don't count toward the exit code, `--max-errors` or `--max-warnings`. `sqlsift check` prints how many were hidden, and a note when baseline entries no longer occur: the problem was fixed, or the entry's file was deleted, renamed or is now ignored. Run `sqlsift check --prune-baseline` to remove them. That note doesn't fail the check unless `--strict-baseline` is given (see below). Entries of files that still exist but weren't checked in this run (say, a pre-commit hook checking only changed files) are never reported as stale.

A diagnostic matches a baseline entry by its file (relative to the baseline file, with symbolic links resolved), rule code and the statement it is in. The statement is compared ignoring comments, whitespace (including whitespace around operators, commas and parentheses), the case of keywords and unquoted identifiers, and quotes around a lowercase identifier (`"users"` is `users`); string literals must be unchanged. In TypeScript and JavaScript files only the SQL template counts, not the code around it, and in Rust, Python and Go files only the query's string literal. So adding lines or statements elsewhere in the file, or running a formatter over the statement, keeps the match. Each entry hides one diagnostic, so the same mistake made twice in one statement needs two entries.

When a statement with baselined problems is changed, its diagnostics that are left are still matched by rule code and message against the file's unmatched entries: fixing one of several problems in a statement doesn't make the others new. The entries store no line numbers and are sorted by file, rule, statement hash and message, so adding lines to a file doesn't change the baseline (and doesn't cause merge conflicts in it).

`--write-baseline` records the diagnostics of the files it checks, and keeps the existing entries of other files that still exist and aren't ignored, printing how many were kept and removed. So it can be run over a few files, and two configurations can share one baseline file. To start over, delete the file first.

### Keeping the baseline shrinking

A baseline should only get smaller. These options of `sqlsift check` help keep it that way:

- `--prune-baseline` removes the entries that no longer occur (problems fixed, files deleted, renamed or ignored) and exits `0`. Unlike `--write-baseline`, it never adds entries, so new problems stay reported.
- `--strict-baseline` fails the check (exit `1`) when entries no longer occur, so whoever fixes a problem also removes its entry, with `--prune-baseline`.
- `--max-baseline-entries <N>` fails the check when the baseline has more than `N` entries, so that a pull request can't grow it by re-running `--write-baseline`. Lower `N` as the baseline shrinks.
- `--write-baseline --baseline-rule <RULE>` rewrites the entries of the given rules only (codes or names, repeatable) and keeps the others, e.g. to record the existing problems of a rule you just turned on without hiding new problems of the other rules.

```bash
# CI
sqlsift check --strict-baseline --max-baseline-entries 140
```

`sqlsift baseline stats` counts the entries by rule and by file (see the [command line reference](../reference/cli.md#sqlsift-baseline-stats)). In `--format json` output a `baseline` object gives the number of diagnostics hidden and of stale entries, and in SARIF output the hidden diagnostics are listed as suppressed results (see [Output formats](../reference/output-formats.md#sarif)).

Baseline files written by sqlsift 0.1 (format version 1) are still read; `--write-baseline` writes version 2.

## Project-wide: rule levels

To turn a rule off everywhere, set its level instead; see [Rules and levels](rules.md).
