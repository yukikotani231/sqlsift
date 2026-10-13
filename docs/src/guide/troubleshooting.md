# Troubleshooting

## A table or column "not found" that does exist

1. Run `sqlsift schema` with the same options (or the same `sqlsift.toml`) to see what sqlsift loaded. If the table is missing, the statement that creates it was probably skipped; skipped statements are listed as warnings on stderr.
2. Check the order of your schema files. With `--schema-dir`, files are applied in version order: numeric prefixes and Flyway `V<version>__` names compare as numbers, so `2_create_table.sql` runs before `10_add_column.sql`. Each directory is sorted on its own, files first and then its subdirectories in name order. Run `sqlsift schema` to see what was loaded.
3. Check the dialect. A MySQL schema parsed as PostgreSQL may be partly skipped.
4. In PostgreSQL, a table in a schema other than `public` must be schema-qualified (`analytics.daily_signups`) unless a `SET search_path` in the file puts its schema on the path, as in the database. See [Schemas and qualified names](sql-support.md#schemas-and-qualified-names).

## "Pattern matched no files" (exit code 2)

Glob patterns are relative to the current directory on the command line, and to the directory of `sqlsift.toml` inside it. Quote patterns containing `**` so your shell doesn't expand them first.

## Seeing what sqlsift is doing

`-v` logs the files and configuration sqlsift uses to stderr; `-vv` adds debug output, such as which files were ignored.

## Valid SQL reported as an error

That's a bug. Suppress it for now with a [`sqlsift:disable`](suppression.md) comment, and please [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with a minimal schema and query. The [playground](https://sqlsift.dev/) is handy for cutting the example down.
