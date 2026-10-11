# Dialects and SQL support

## Dialects

| Dialect | Flag | Notes |
|---------|------|-------|
| PostgreSQL | default, `--dialect postgresql` | Most complete: enums, `DISTINCT ON`, `LATERAL`, JSON operators, psql scripts |
| MySQL | `--dialect mysql` | Backtick identifiers, inline `ENUM(...)`, `AUTO_INCREMENT`, `mysqldump` files, `INSERT ... SET`, index hints ([details](queries.md#mysql-syntax)); booleans are integers |
| SQLite | `--dialect sqlite` | SQLite's loose typing: booleans are integers |

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

## Schemas and qualified names

Tables, views and enum types are kept per schema, so `auth.users` and `public.users` are two tables (as in Supabase projects). Names can be written as `table`, `schema.table` or `database.schema.table`, and columns as `table.column`, `schema.table.column` or `database.schema.table.column`; the database part is not checked.

- **PostgreSQL**: an unqualified name is looked up in the `search_path` (`public` unless a `SET search_path` in the file changes it). A table in a schema off the path must be qualified: `SELECT * FROM daily_signups` is [E0001](../rules/E0001.md) with a hint to write `analytics.daily_signups`. `schema.table.column` must name a table of the `FROM` clause, so `auth.users.email` is reported when `FROM users` is `public.users`.
- **MySQL**: databases act as schemas (`shop.orders`, `USE shop;` in schema and query files). The current database is chosen when the query runs, so an unqualified name is found in any database, and a database name the schema doesn't use (a dump without `USE`) matches the tables created without one.
- **SQLite**: `main.users` and `temp.users` refer to the tables of the schema; tables created as `archive.users` belong to the attached database `archive`.

A backtick-quoted name containing dots (`` `shop.orders` ``) is split into its parts. A double-quoted one (`"a.b"`) stays one name, as in PostgreSQL.

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

- Function bodies and stored procedures are skipped, not analyzed.
- SQL embedded in application code is supported for tagged template literals in TypeScript, JavaScript, Vue and Svelte files (see [SQL in TypeScript and JavaScript](queries.md#sql-in-typescript-and-javascript)); strings in other languages (Python, Go, …) are not.
- dbt / Jinja templates are masked, not rendered (see [dbt and Jinja templates](queries.md#dbt-and-jinja-templates)): macros aren't expanded, and the columns of `{{ ref(...) }}` models are unknown.
