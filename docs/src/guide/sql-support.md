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

In BigQuery, the fields of a STRUCT and of an `UNNEST` element are not modelled, so they are never reported, and a query that uses `UNNEST` doesn't report unqualified column names (they may be fields of the element). Table names are matched case-insensitively. Quote a wildcard table with backticks (`` `dataset.events_*` ``): unquoted, it doesn't parse yet.

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
