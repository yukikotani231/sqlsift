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

## dbt and Jinja templates

dbt models are Jinja templates, not plain SQL. With `--templating jinja` (or `templating = "jinja"` in `sqlsift.toml`) sqlsift masks the template syntax before checking a query file. This is turned on automatically when a `dbt_project.yml` is in the current directory or in the directory of `sqlsift.toml`; set `templating = "none"` to turn it off.

```sql
{{ config(materialized='incremental') }}

select c.id, c.frist_name, o.amount       -- E0002: 'frist_name' is checked against customers
from customers c
join {{ ref('stg_orders') }} o on o.customer_id = c.id   -- o.amount: not reported
{% if is_incremental() %}
where c.id > (select max(customer_id) from {{ this }})
{% endif %}
```

- `{# comments #}` and `{% statements %}` are skipped. The SQL inside `{% if %}` and `{% for %}` blocks is checked once, as written.
- `{{ ref(...) }}`, `{{ source(...) }}`, `{{ this }}` and any other `{{ ... }}` where a table name is expected (after `FROM`, `JOIN`, `INTO`, `UPDATE`, `USING`) is a table whose columns sqlsift doesn't know: it is not reported as missing, and neither are columns qualified by it or unqualified columns that may come from it. Put the tables your models read from (the dbt sources) in the schema to get them checked.
- `{{ ... }}` as part of a name (`total_{{ c }}`, `as {{ alias }}`) is a name sqlsift doesn't know, and at the start of a statement (`{{ config(...) }}`) it is skipped.
- Any other `{{ ... }}` is an untyped value, like a bind parameter: it is never a type mismatch.
- Everything else is checked as usual, and diagnostics point at the original file.

Macros that expand to whole clauses or statements can't be followed, and both branches of an `{% if %} ... {% else %}` are kept, which may not be valid SQL. Use `ignore` or `-- sqlsift:disable-file` for such files.

sqlsift supports the PostgreSQL, MySQL and SQLite dialects only, so this helps dbt projects on Postgres (or a Postgres-compatible warehouse such as Redshift, as far as its SQL is Postgres-compatible), MySQL or SQLite. Projects on Snowflake, BigQuery or Databricks won't benefit until those dialects are supported.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | No errors (warnings may have been reported) |
| `1` | At least one error, or more warnings than `--max-warnings` / `max_warnings` |
| `2` | Usage or configuration error: missing files, a pattern that matches no files, an invalid `sqlsift.toml`, … |
