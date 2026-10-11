# Dialects and SQL support

## Dialects

| Dialect | Flag | Notes |
|---------|------|-------|
| PostgreSQL | default, `--dialect postgresql` | Most complete: enums, `DISTINCT ON`, `LATERAL`, JSON operators, psql scripts |
| MySQL | `--dialect mysql` | Backtick identifiers, inline `ENUM(...)`, `AUTO_INCREMENT`, `mysqldump` files, `INSERT ... SET`, index hints ([details](queries.md#mysql-syntax)); booleans are integers |
| SQLite | `--dialect sqlite` | SQLite's loose typing: booleans are integers |
| Snowflake (alpha) | `--dialect snowflake` | `QUALIFY`, lateral column aliases (`SELECT a + 1 AS b ... WHERE b > 0`), semi-structured paths (`v:a.b`) parse |
| BigQuery (alpha) | `--dialect bigquery` | Backtick identifiers, `QUALIFY`, `SELECT * EXCEPT / REPLACE`, `UNNEST` parse; `_PARTITIONTIME` and friends are known |
| Redshift (alpha) | `--dialect redshift` | PostgreSQL-like, with lateral column aliases and Redshift system tables |
| Databricks (alpha) | `--dialect databricks` | Spark SQL / Databricks SQL, backtick identifiers, lateral column aliases |

The data warehouse dialects are alpha: queries parse with the warehouse's syntax and table and column names are checked, but many warehouse types (`VARIANT`, `STRUCT`, ...) and functions are not known yet. Like everything sqlsift can't work out, they are left unreported rather than guessed. For dbt projects on a warehouse, see [dbt and Jinja templates](queries.md#dbt-and-jinja-templates).

Set it once in `sqlsift.toml` with `dialect = "mysql"`.

## Statements

- `SELECT`, `INSERT`, `UPDATE`, `DELETE`, including `RETURNING`
- JOINs (`INNER`, `LEFT`, `RIGHT`, `FULL`, `CROSS`, `NATURAL`) with `ON` / `USING`
- CTEs (`WITH`), including recursive CTEs
- Subqueries: `IN` / `EXISTS`, derived tables in `FROM`, scalar subqueries
- `UNION` / `INTERSECT` / `EXCEPT`, with column count and type checks
- Window functions (`OVER`, `PARTITION BY`, frames), aggregate `FILTER`
- `GROUPING SETS`, `CUBE`, `ROLLUP`, `DISTINCT ON`
- Expressions: `CASE`, `CAST`, `EXTRACT`, JSON operators, `AT TIME ZONE`, `ARRAY`, …

## Type checking

sqlsift infers expression types to report [E0003](../rules/E0003.md) and [E0007](../rules/E0007.md). It currently understands:

- Comparisons and arithmetic in `WHERE`, `SELECT`, `JOIN ... ON`, nested expressions
- `INSERT ... VALUES` and `UPDATE ... SET` values against the column type
- Numeric widening (`SMALLINT` → `INTEGER` → `BIGINT` → `NUMERIC`)
- String literals coerce to the other side's type like in the database (`created_at > '2024-01-01'`, `id = '42'` are fine), while impossible values are still reported (`id = 'abc'`)
- Date and time arithmetic (`now() - interval '7 days'`, `placed_on + 7`)
- `CAST`, and the return types of common functions (`COUNT`, `SUM`, `AVG`, `UPPER`, `LENGTH`, `COALESCE`, …)
- `CASE` branch consistency and result type
- Enum values for PostgreSQL enum types and MySQL inline `ENUM(...)`, with "did you mean" suggestions
- Column types through CTEs, subqueries, views and `CREATE TABLE ... AS`

Anything sqlsift can't infer is treated as unknown and never reported, so missing type support leads to missed errors, not false positives.

## Known limitations

- Schema-qualified names (`public.users`) are not fully resolved.
- Function bodies and stored procedures are skipped, not analyzed.
- SQL embedded in application code is supported for tagged template literals in TypeScript, JavaScript, Vue and Svelte files (see [SQL in TypeScript and JavaScript](queries.md#sql-in-typescript-and-javascript)); strings in other languages (Python, Go, …) are not.
- dbt / Jinja templates are masked, not rendered (see [dbt and Jinja templates](queries.md#dbt-and-jinja-templates)): macros aren't expanded, and the columns of `{{ ref(...) }}` models are unknown.
