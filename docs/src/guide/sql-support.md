# Dialects and SQL support

## Dialects

| Dialect | Flag | Notes |
|---------|------|-------|
| PostgreSQL | default, `--dialect postgresql` | Most complete: enums, `DISTINCT ON`, `LATERAL`, JSON operators, psql scripts |
| MySQL | `--dialect mysql` | Backtick identifiers, inline `ENUM(...)`, `AUTO_INCREMENT`, `mysqldump` files, `INSERT ... SET`, index hints ([details](queries.md#mysql-syntax)); booleans are integers |
| SQLite | `--dialect sqlite` | SQLite's loose typing: booleans are integers |
| Snowflake (alpha) | `--dialect snowflake` | Types (`NUMBER(p, s)`, `TIMESTAMP_NTZ` / `_LTZ` / `_TZ`, `VARIANT` / `OBJECT` / `ARRAY` / `GEOGRAPHY` never type checked, nor `v:a.b` / `v['k']` paths), return types of common functions (`IFF`, `NVL`, `DECODE`, `ZEROIFNULL`, `DIV0`, `TRY_TO_*`, `DATEADD`, `DATEDIFF`, `COUNT_IF`, …), date parts as bare words (`DATEADD(day, ...)`), `LATERAL FLATTEN` columns (`SEQ`, `KEY`, `PATH`, `INDEX`, `VALUE`, `THIS`), `SELECT * EXCLUDE / RENAME / REPLACE`, `GROUP BY ALL`, `IDENTIFIER('table')`, `QUALIFY`, lateral column aliases (`SELECT a + 1 AS b ... WHERE b > 0`), self-referencing CTEs without `RECURSIVE`, positional columns (`$1`, `t.$1`). Snowflake Scripting blocks (`DECLARE ... BEGIN ... END;`) and administration statements (`ALTER SESSION`, `ALTER WAREHOUSE`, `CREATE TASK`, `GRANT ... ON WAREHOUSE`, ...) are skipped without a parse error. Strings convert implicitly like in Snowflake (`varchar_col = number_col` is fine, `number_col = 'abc'` is reported); unquoted and quoted names match case-insensitively. Stage queries (`FROM @stage`) and time travel (`AT(...)` / `BEFORE(...)`) don't parse yet |
| BigQuery (alpha) | `--dialect bigquery` | `` `project.dataset.table` `` names (a dataset the schema doesn't define, or a wildcard table `` `events_*` ``, is never reported), BigQuery types (`INT64`, `STRING`, `NUMERIC`, `STRUCT<...>`, `ARRAY<...>`, `JSON`, ...), `UNNEST(...) [WITH OFFSET]`, STRUCT field paths (`o.shipping.city`), `SELECT * EXCEPT / REPLACE` columns, `QUALIFY`, return types of common functions (`SAFE_CAST`, `SAFE_DIVIDE`, `DATE_DIFF`, `FORMAT_DATE`, `COUNTIF`, `JSON_VALUE`, `SAFE.` prefix, ...) and date parts (`DAY`, `ISOWEEK`, `WEEK(MONDAY)`); `_PARTITIONTIME` and `_TABLE_SUFFIX` are known |
| Redshift (alpha) | `--dialect redshift` | PostgreSQL-like, with lateral column aliases and Redshift system tables. `CREATE TABLE` attributes (`DISTKEY`, `SORTKEY`, `COMPOUND` / `INTERLEAVED SORTKEY`, `DISTSTYLE`, `ENCODE`, `BACKUP`, `IDENTITY(seed, step)`), date parts as bare words (`DATE_PART(h, ts)`, `DATEDIFF(day, a, b)`), `SYSDATE` and `CURRENT_USER` without parentheses |
| Databricks (alpha) | `--dialect databricks` | Spark SQL / Databricks SQL, backtick identifiers, lateral column aliases. `MAP<...>` / `STRUCT<a: T, ...>` / `ARRAY<...>` column types (struct fields `s.a` are not checked), `LONG` / `SHORT` / `BYTE`, table options (`USING DELTA`, `PARTITIONED BY`, `CLUSTER BY`, `OPTIONS`, `LOCATION`, `TBLPROPERTIES`), `LATERAL VIEW [OUTER] explode(...) alias AS c, ...` |

The data warehouse dialects are alpha: queries parse with the warehouse's syntax and table and column names are checked, but many warehouse types (`VARIANT`, `STRUCT`, ...) and functions are not known yet. Like everything sqlsift can't work out, they are left unreported rather than guessed. For dbt projects on a warehouse, see [dbt and Jinja templates](queries.md#dbt-and-jinja-templates).

In BigQuery, the fields of a STRUCT and of an `UNNEST` element are not modelled, so they are never reported (`struct_col.*` and `f(x).*` have unknown columns), and a query that uses `UNNEST` doesn't report unqualified column names (they may be fields of the element). Table names are matched case-insensitively. Quote a wildcard table with backticks (`` `dataset.events_*` ``): unquoted, it doesn't parse yet.

When the schema names projects (`` CREATE TABLE `my-project.dataset.t` ``), a table of another project is unknown and never reported, even if a dataset of the schema has a table with its name; a schema that names no project matches any project. In scripts, variables declared with `DECLARE` are known names in the statements after it. A `GROUP BY`, `HAVING`, `QUALIFY` or `ORDER BY` name that is an output column (`SELECT a.uid AS uid ... GROUP BY uid`, or the implicit name of `a.uid`) is that column, as in BigQuery. `UNPIVOT` output columns are checked; `PIVOT` columns are unknown. `ANY TYPE` function parameters, `<<` / `>>`, field access on function results (`f(x).field`) and typed array literals (`` ARRAY<STRUCT<a INT64>>[...] ``) are supported; procedural blocks (`FOR ... IN`, `BEGIN ... EXCEPTION`, `EXECUTE IMMEDIATE`) don't parse yet.

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

sqlsift infers expression types to report [E0003](../rules/E0003.md), [E0007](../rules/E0007.md), [E0017](../rules/E0017.md) and [E0027](../rules/E0027.md). It currently understands:

- Comparisons and arithmetic in `WHERE`, `SELECT`, `JOIN ... ON`, nested expressions
- `INSERT ... VALUES` and `UPDATE ... SET` values against the column type
- Numeric widening (`SMALLINT` → `INTEGER` → `BIGINT` → `NUMERIC`)
- String literals coerce to the other side's type like in the database (`created_at > '2024-01-01'`, `id = '42'` are fine), while impossible values are still reported (`id = 'abc'`)
- Date and time arithmetic (`now() - interval '7 days'`, `placed_on + 7`)
- `CAST`, and the return types of common functions (`COUNT`, `SUM`, `AVG`, `UPPER`, `LENGTH`, `COALESCE`, …)
- `CASE` branch consistency and result type
- Enum values for PostgreSQL enum types and MySQL inline `ENUM(...)`, with "did you mean" suggestions
- Column types through CTEs, subqueries, views and `CREATE TABLE ... AS`
- Argument types of built-in functions and `LIKE` ([E0027](../rules/E0027.md)), and literal lengths and ranges against the column type ([E0017](../rules/E0017.md))

Anything sqlsift can't infer is treated as unknown and never reported, so missing type support leads to missed errors, not false positives.

MySQL and SQLite convert mismatched values at run time instead of rejecting the query, so with these dialects type mismatches are the warning [E0029](../rules/E0029.md), not errors; only a string literal MySQL rejects when assigning it to a column stays [E0003](../rules/E0003.md).

## Known limitations

- Function bodies and stored procedures are skipped, not analyzed.
- SQL embedded in application code is supported for tagged template literals in TypeScript, JavaScript, Vue and Svelte files (see [SQL in TypeScript and JavaScript](queries.md#sql-in-typescript-and-javascript)) sqlx queries in Rust (see [SQL in Rust (sqlx)](queries.md#sql-in-rust-sqlx)), and string literals passed to database calls in Python (see [SQL in Python](queries.md#sql-in-python)) and Go (see [SQL in Go](queries.md#sql-in-go)); strings in other languages (Java, Ruby, …) are not. SQL those libraries read from `.sql` files (sqlc, sqlx `query_file!`, aiosql) is checked as it is; see the [example projects](../integrations/examples.md).
- dbt / Jinja templates are masked, not rendered (see [dbt and Jinja templates](queries.md#dbt-and-jinja-templates)): macros aren't expanded, and the columns of `{{ ref(...) }}` models are unknown unless dbt's `catalog.json` is there (see [Columns of models and sources](queries.md#columns-of-models-and-sources-from-dbts-catalog-alpha), alpha).
