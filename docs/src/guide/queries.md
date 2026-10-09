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

## File encoding

Query and schema files are read as UTF-8. A byte order mark at the start of a file (written by many Windows editors, SSMS and DBeaver exports) is ignored, both in files and on stdin. Columns in diagnostics don't count it, matching what an editor shows.

Very long expressions, such as generated `WHERE id = 1 OR id = 2 OR ...` filters with tens of thousands of terms, are fine: analysis runs with a large stack.

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

sqlc's named parameters are untyped placeholders, like `$1`:

```sql
-- name: ListPosts :many
SELECT id, title FROM posts
WHERE id > @after_id AND author_id = sqlc.arg(author_id)
  AND (title = sqlc.narg('title') OR sqlc.narg('title') IS NULL)
LIMIT @page_size;

-- name: GetPostsByIDs :many
SELECT id, title FROM posts WHERE id IN (sqlc.slice(ids));
```

- `sqlc.arg(name)`, `sqlc.narg(name)` and `sqlc.slice(name)` (with the name bare or quoted) are placeholders in every dialect.
- `@name` is a placeholder with the PostgreSQL dialect only. PostgreSQL's `@` operators are left alone: `@>`, `<@`, `@@`, and `@` followed by a space (absolute value). With MySQL, `@name` stays a user variable (`SET @x = 1`), and with SQLite a bind parameter; sqlc supports `@name` for neither, so use `sqlc.arg(name)` there.
- Parameters in string literals, quoted identifiers and comments are left alone.

## SQL in TypeScript and JavaScript

Files ending in `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts`, `.mjs` or `.cjs` are checked for SQL in tagged template literals, as used by postgres.js, Slonik, kysely, `@vercel/postgres`, Prisma and others. In Vue (`.vue`) and Svelte (`.svelte`) components, the `<script>` blocks are checked the same way:

```ts
const posts = await sql`
  SELECT id, titel FROM posts WHERE author_id = ${authorId}
`;
```

```bash
sqlsift check -s schema.sql 'src/**/*.ts'
```

Diagnostics point at the query's line and column in the source file. When a glob pattern matches TypeScript or JavaScript files in `node_modules`, `dist`, `build`, `.next`, `.nuxt` or `.svelte-kit` directories (installed packages and build output), they are skipped; name such a file, or start the pattern in such a directory (`'dist/**/*.js'`), to check it anyway.

### Tags

Which templates are SQL is decided by their tag: `embedded_sql_tags` in `sqlsift.toml` lists the tags (default `["sql"]`). The tag expression may be a chain of member accesses and calls, and matches when its last or its first identifier is one of the tags:

| Tag expression | Matches `"sql"` because of |
|---|---|
| `` sql`...` ``, `` db.sql`...` ``, `` Prisma.sql`...` `` | the last identifier |
| `` sql.unsafe`...` ``, `` sql.type(schema)`...` ``, `` sql.typeAlias('id')`...` `` (Slonik) | the first identifier |

Type arguments are skipped (`` sql<boolean>`...` ``, `` prisma.$queryRaw<User[]>`...` ``). Tags per library:

| Library | `embedded_sql_tags` |
|---|---|
| postgres.js, Slonik, kysely, `@vercel/postgres`, `sql-template-strings` | `["sql"]` (the default) |
| Prisma | `["$queryRaw", "$executeRaw"]` (and `"sql"` for `Prisma.sql` fragments) |

```toml
embedded_sql_tags = ["sql", "$queryRaw", "$executeRaw"]
```

### Statements and fragments

A template is checked only when it starts with a statement keyword (`SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE`, `VALUES`, `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `MERGE`, ..., after comments and opening parentheses). Other templates written with the same tag are query fragments, such as kysely's `` sql`published = ${x}` ``, `` Prisma.sql`WHERE id > ${minId}` `` or `` sql`AND published` ``, and are skipped. (A misspelled statement keyword, as in `` sql`SELEC id FROM posts` ``, is still checked and reported as a syntax error.)

Each checked template is one statement:

- `${expr}` is an untyped placeholder, like `$1` (`?` for MySQL and SQLite), or a parenthesized list after `IN`.
- `${expr}` where a table name is expected (after `FROM`, `JOIN`, `INTO`, `UPDATE` or `TABLE`) is a name sqlsift can't know, so no "not found" diagnostic is reported for it, as for psql's `:"var"`.
- `${expr}` after a value or a name is a fragment between clauses, and is left out: `` WHERE a = ${a} ${cond ? sql`AND b` : sql``} ORDER BY id ``, or Prisma's `` SELECT id FROM users ${where} ``.
- postgres.js helpers are rows and columns sqlsift can't know: `` INSERT INTO users ${sql(user, 'name')} ``, `` INSERT INTO posts (a, b) VALUES ${sql(rows)} `` and `` UPDATE users SET ${sql(patch)} WHERE ... `` are checked without their column lists.
- Templates inside another SQL template's `${...}` are fragments and are not checked on their own.
- SQL built by string concatenation, and untagged templates, are not checked.

`--stdin-filename` with a TypeScript, JavaScript, Vue or Svelte extension checks stdin the same way.

### Directives

[Suppression comments](suppression.md) work as code comments (`// sqlsift:disable-file`, `/* sqlsift:disable E0002 */`) and as SQL comments inside a template. A `sqlsift:disable` comment on a line of its own applies to the next line of SQL (for a template starting on the next line, its first line of SQL); after a query, it applies to that line. A block comment followed by code on the same line is ignored. A `sqlsift:disable-file` comment applies to the whole file, also when it is written inside one of its templates.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | No errors (warnings may have been reported) |
| `1` | At least one error, or more warnings than `--max-warnings` / `max_warnings` |
| `2` | Usage or configuration error: missing files, a pattern that matches no files, an invalid `sqlsift.toml`, … |
