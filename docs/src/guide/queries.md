# Checking queries

`sqlsift check` parses each query file, resolves every table and column reference against the schema, and infers expression types to find mismatches.

## Choosing files

Query files come from the command line or from `files` in `sqlsift.toml`. Both accept glob patterns; `**` matches any number of directories.

```bash
sqlsift check 'queries/**/*.sql'
```

```toml
files = ["app/queries/**/*.sql", "db/reports/*.sql"]
```

To skip files, add `ignore` patterns (or `--ignore` on the command line). They apply to files from both sources:

```toml
ignore = ["queries/archive/**", "**/*.generated.sql"]
```

A pattern that matches a directory skips everything below it. Patterns in `sqlsift.toml` are relative to the file's directory; `--ignore` patterns are relative to the current directory and add to the file's list.

## Reading from stdin

Pass `-` as the file name to read a single query from stdin. `--stdin-filename` sets the file name shown in diagnostics (default `<stdin>`):

```bash
git show :queries/users.sql | sqlsift check -s schema.sql --stdin-filename queries/users.sql -
```

This is how editors and [pre-commit hooks](../integrations/pre-commit.md) can check content that isn't saved on disk.

## What a query can see

sqlsift follows SQL's visibility rules rather than just matching names:

- Table aliases, CTEs (including recursive CTEs) and derived tables
- Correlated subqueries in `WHERE`, `SELECT` and `HAVING`
- `LATERAL` vs non-`LATERAL` subqueries in `FROM`
- `JOIN ... USING` and `NATURAL JOIN` columns
- `ORDER BY` references to `SELECT` aliases
- `UPDATE ... FROM` and `DELETE ... USING`
- Table-valued functions in `FROM` (for example `generate_series`)

## DDL inside query files

Query files can create their own tables. `CREATE [TEMP] TABLE`, `CREATE TABLE ... AS SELECT`, `CREATE VIEW`, `ALTER TABLE` and `DROP` statements in a query file apply to the later statements of **that file only**:

```sql
CREATE TEMP TABLE recent_orders AS
SELECT id, user_id FROM orders WHERE created_at > now() - interval '7 days';

SELECT user_id, count(*) FROM recent_orders GROUP BY user_id;  -- OK
```

## psql scripts

With the PostgreSQL dialect, files written for `psql` are accepted:

- Backslash meta-commands (`\set`, `\i`, `\connect`, `\if`, …) are skipped.
- `\g`, `\gset` and `\gx` end a query like `;`.
- `:var` and `:'var'` interpolations are treated as untyped placeholders, and `:"var"` as an identifier whose name sqlsift can't know, so no "not found" diagnostic is reported for it.

## sqlc query files

[sqlc](https://sqlc.dev) query files are plain SQL with a `-- name:` comment before each query, so they can be checked as they are. Diagnostics name the query they are in:

```sql
-- name: ListPosts :many
SELECT id, titel FROM posts WHERE author_id = $1;
```

```text
error[E0002]: Column 'titel' not found in table 'posts'
  --> queries/posts.sql:2:12
    |
  2 | SELECT id, titel FROM posts WHERE author_id = $1;
    |            ^^^^^
    = note: in query 'ListPosts'
    = help: Did you mean 'title'?
```

A statement belongs to the last `-- name: <Name> :<command>` comment before it. The name is the `query_name` field in JSON output, a logical location in SARIF output, and is added to the message in SARIF, `github` and editor diagnostics.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | No errors (warnings may have been reported) |
| `1` | At least one error, or more warnings than `--max-warnings` / `max_warnings` |
| `2` | Usage or configuration error: missing files, a pattern that matches no files, an invalid `sqlsift.toml`, … |
