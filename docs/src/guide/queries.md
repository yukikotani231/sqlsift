# Checking queries

`sqlsift check` parses each query file, resolves every table and column reference against the schema, and infers expression types to find mismatches.

## Choosing files

Query files come from the command line or from `files` in `sqlsift.toml`. Both accept glob patterns; `**` matches any number of directories. Directories a pattern matches are skipped (`'queries/**/*'` checks the files under `queries/`); a directory named on its own is an error.

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

Query and schema files are read as UTF-8, or as UTF-16 when they start with a UTF-16 byte order mark (SSMS's "Unicode" encoding). A byte order mark at the start of a file (written by many Windows editors, SSMS and DBeaver exports) is ignored, both in files and on stdin. Columns in diagnostics don't count it, matching what an editor shows.

A query file in any other encoding (Latin-1, Shift_JIS, UTF-16 without a byte order mark, binary files) is skipped with a warning, and the other files are still checked. A schema file sqlsift can't read stops the check, since every query depends on it.

Very long expressions, such as generated `WHERE id = 1 OR id = 2 OR ...` filters with tens of thousands of terms, are fine: analysis runs with a large stack.

## What a query can see

sqlsift follows SQL's visibility rules rather than just matching names:

- Table aliases, CTEs (including recursive CTEs) and derived tables
- Correlated subqueries in `WHERE`, `SELECT` and `HAVING`
- `LATERAL` vs non-`LATERAL` subqueries in `FROM`
- `JOIN ... USING` and `NATURAL JOIN` columns
- `ORDER BY` references to `SELECT` aliases (also in `HAVING` with MySQL and SQLite, which allow it; PostgreSQL doesn't)
- `UPDATE ... FROM` and `DELETE ... USING`
- Table-valued functions in `FROM` (for example `generate_series`)

## DDL inside query files

Query files can create their own tables. `CREATE [TEMP | UNLOGGED] TABLE`, `CREATE TABLE ... AS SELECT [WITH [NO] DATA]`, `SELECT ... INTO [TEMP] t`, `CREATE VIEW`, `ALTER TABLE` and `DROP` statements in a query file apply to the later statements of **that file only**:

```sql
CREATE TEMP TABLE recent_orders AS
SELECT id, user_id FROM orders WHERE created_at > now() - interval '7 days';

SELECT user_id, count(*) FROM recent_orders GROUP BY user_id;  -- OK
```

If a `CREATE TABLE` or `CREATE VIEW` can't be parsed, the "table not found" errors for it later in the file say so.

`SET search_path TO analytics, public` (also `SET LOCAL search_path`, `SET search_path = ...`) is followed for the rest of the file: unqualified names are looked up in the listed schemas, in order, and tables created without a schema go in the first one. `SET search_path TO DEFAULT` goes back to the default schema.

## psql scripts

With the PostgreSQL dialect, files written for `psql` are accepted:

- Backslash meta-commands (`\set`, `\i`, `\connect`, `\if`, …) are skipped.
- `\g`, `\gset` and `\gx` end a query like `;`.
- `:var` and `:'var'` interpolations are treated as untyped placeholders, and `:"var"` as an identifier whose name sqlsift can't know, so no "not found" diagnostic is reported for it.
- The data of a `COPY ... FROM stdin;` (the lines up to `\.`, as in `pg_dump` output and seed files) is skipped, in query files and schema files.

## MySQL syntax

With the MySQL dialect, these are accepted in schema and query files:

- Versioned comments (`/*!50001 ... */`, as written by `mysqldump`) are read as SQL, the way MySQL runs them. Views in a dump are loaded.
- `ALGORITHM = ...`, `DEFINER = ...` and `SQL SECURITY ...` in `CREATE VIEW` are ignored.
- `INSERT ... SET col = value, ...` is checked like `INSERT ... (col, ...) VALUES (value, ...)`, including `ON DUPLICATE KEY UPDATE`.
- Index hints (`USE`, `FORCE` and `IGNORE INDEX`/`KEY`), `STRAIGHT_JOIN` and the SELECT modifiers (`SQL_CALC_FOUND_ROWS`, `SQL_NO_CACHE`, `HIGH_PRIORITY`, `DISTINCTROW`, ...) are ignored.

## dbt and Jinja templates

dbt models are Jinja templates, not plain SQL. With `--templating jinja` (or `templating = "jinja"` in `sqlsift.toml`) sqlsift masks the template syntax before checking a query file. This is turned on automatically when a `dbt_project.yml` is in the current directory, in the directory of `sqlsift.toml`, or in a directory above the query file (so `sqlsift check -s schema.sql 'analytics/models/**/*.sql'` works from a monorepo root); set `templating = "none"` to turn it off. When a file with `{{` or `{%` fails to parse without templating, the error suggests `--templating jinja`.

```sql
{{ config(materialized='incremental') }}

select c.id, c.frist_name, o.amount       -- E0002: 'frist_name' is checked against customers
from customers c
join {{ ref('stg_orders') }} o on o.customer_id = c.id   -- o.amount: not reported
{% if is_incremental() %}
where c.id > (select max(customer_id) from {{ this }})
{% endif %}
```

- `{# comments #}` and `{% statements %}` are skipped. The SQL inside a `{% for %}` block is checked once, as a single iteration (both `loop.first` and `loop.last`): separators such as `{% if not loop.last %},{% endif %}` and `{{ ',' if not loop.last }}` are dropped.
- Of `{% if %} ... {% elif %} ... {% else %} ... {% endif %}` only one branch is checked; the others are skipped. It is the first branch whose condition may be true: sqlsift evaluates `is_incremental()` (false, as on a model's first build), `var('x', true)` / `var('x', false)` (the default), `target.type == 'snowflake'` / `!=` / `in [...]` (against the dialect's dbt adapter), `{% set x = ... %}` variables holding such a value, and `not` / `and` / `or` of these. Any other condition counts as true.
- The bodies of `{% set x %}...{% endset %}`, `{% call %}...{% endcall %}`, `{% macro %}...{% endmacro %}` (and `test`, `materialization`, `docs` blocks) are skipped. The body of `{% raw %}...{% endraw %}` is checked as SQL.
- `{{ source('raw', 'customers') }}` is the schema's table `raw.customers` when your schema has it, so its columns are checked.
- `{{ ref('orders') }}` and `{{ source('shop', 'orders') }}` have the columns of the model or source in dbt's `catalog.json`, when there is one (see [Columns of models and sources](#columns-of-models-and-sources-from-dbts-catalog-alpha) below).
- Any other `{{ ref(...) }}` or `{{ source(...) }}`, `{{ this }}` and any other `{{ ... }}` where a table name is expected (after `FROM`, `JOIN`, `INTO`, `UPDATE`, `USING`) is a table whose columns sqlsift doesn't know: it is not reported as missing, and neither are columns qualified by it or unqualified columns that may come from it. Put the tables your models read from (the dbt sources) in the schema, or generate dbt's catalog, to get them checked.
- `{{ ... }}` as part of a name (`total_{{ c }}`, `as {{ alias }}`) is a name sqlsift doesn't know: an output column named by it has an unknown name, and a table alias built from it in a loop (`left join o as {{ f }}_o`) makes qualifiers of that shape (`a_o.v`) unknown.
- `{{ ... }}` at the start of a statement, after any comments (`{{ config(...) }}`), is skipped. When a query follows it directly, the macro may write the query's `WITH` clause, so table names in that query that aren't in the schema are not reported.
- `{{ ... }}` right after a complete expression is a macro that adds list items or a whole clause. In a `SELECT` list (`select id {{ fivetran_utils.apply_source_relation() }} from t`) it adds columns sqlsift doesn't know: the columns of that query are unknown to the queries that read it. Elsewhere (`{{ dbt_utils.group_by(2) }}`, `partition by id {{ ... }}`, a macro that writes a `WHERE` clause) it is skipped.
- `{{ ... }}` that is the whole body of a CTE or a subquery in `FROM` (`with spine as ({{ dbt_utils.date_spine(...) }})`) is a query whose columns sqlsift doesn't know.
- `{{ var('x') }}` where a table name is expected is the relation of the dbt project variable `x` when `vars:` in `dbt_project.yml` sets it to a `{{ ref(...) }}` or `{{ source(...) }}` that sqlsift knows (diagnostics name it `var.x`), and `{{ var('x', ref('m')) }}` is the default relation; otherwise it is a table with unknown columns.
- `{{ ... }}` as a type (`x::{{ dbt.type_bigint() }}`, `cast(x as {{ ... }})`) is a type sqlsift doesn't know.
- Any other `{{ ... }}`, and a string literal with one in it (`'{{ var("start") }}'`), is an untyped value, like a bind parameter: it is never a type mismatch.
- `sqlsift:disable` directives work in Jinja comments too: `{# sqlsift:disable-file #}`, `{# sqlsift:disable E0002 #}` (dbt users avoid `--` comments, which end up in the compiled SQL).
- Everything else is checked as usual, and diagnostics point at the original file.

Macros that expand to other SQL can't be followed: a parse error on one shows the template tag (`found: Jinja expression {{ dbt_utils.date_spine(...) }}`). Use `ignore` or `{# sqlsift:disable-file #}` for such files.

### Columns of models and sources from dbt's catalog (alpha)

`dbt docs generate` writes `target/catalog.json`, with the columns and types of every model, seed, snapshot and source as they are in the warehouse. When it exists in the dbt project (next to `dbt_project.yml`), sqlsift reads it, so the columns of `{{ ref(...) }}` and `{{ source(...) }}` are checked too, and schema files become optional:

```console
$ dbt docs generate
$ sqlsift check 'models/**/*.sql'
error[E0002]: Column 'amount_usd' not found in table 'ref.stg_orders'
  --> models/marts/order_typo.sql:3:22
```

Use `--dbt-catalog <PATH>` (or `dbt_catalog = "..."` in `sqlsift.toml`) when the catalog is elsewhere, for example when `target-path` is changed or the catalog is downloaded from a CI artifact or dbt Cloud. It is read whatever the templating, and a configured catalog that is missing or not a dbt catalog is an error; one found in `target/` that can't be read is a warning.

- `{{ ref('orders') }}` is the model, seed or snapshot named `orders`; diagnostics name it `ref.orders`. Its relation is also a table under its warehouse name (`analytics.orders`) for SQL that names it directly. Tables in your schema files take precedence over the catalog.
- `{{ source('shop', 'orders') }}` is the table `orders` of the source `shop`.
- A `ref()` of a model that isn't in the catalog (not built yet when the catalog was generated), a package-qualified `ref('package', 'model')`, a versioned `ref('model', v=2)`, and a model name that two packages use, are tables with unknown columns, as without a catalog.
- Column types are the warehouse's type names. The common ones (`integer`, `int64`, `number`, `varchar`, `string`, `timestamp_ntz`, `boolean`, ...) are checked; others (`variant`, `struct`, arrays, geography types) are unknown types and never a type mismatch.

The catalog is a snapshot of the warehouse when `dbt docs generate` last ran: regenerate it after changing a model's columns, or sqlsift checks the models that use it against the old columns. The language server reads it when it starts. This support is **alpha**: how `ref()` names appear in diagnostics and how the catalog is found may change.

With Jinja templating, files in the dbt project's `macros/`, `dbt_packages/` and `target/` directories are skipped when they are matched by a glob pattern (`'**/*.sql'`); a file named on its own is still checked. Add other directories you don't want checked (`analyses/`, `snapshots/`) to `ignore`.

Set `dialect` to your warehouse. Snowflake, BigQuery, Redshift and Databricks are supported in alpha (see [Dialects](sql-support.md#dialects)): their queries parse and names are checked, but many warehouse types and functions are not known yet, and are left unreported.

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

## sqlx query files

The files [sqlx](https://github.com/launchbadge/sqlx)'s `query_file!` and `query_file_as!` read are plain SQL with `$1` parameters (`?` with MySQL and SQLite), one statement each, so they are checked as they are, against the migrations `sqlx::migrate!()` applies:

```toml
schema_dir = "migrations"         # <timestamp>_name.sql; *.down.sql halves are skipped
files = ["queries/**/*.sql"]
```

sqlx's type overrides in column aliases (`id AS "id!"`, `status AS "status: Status"`) are ordinary quoted aliases. SQL written inline in Rust is checked too; see [SQL in Rust (sqlx)](#sql-in-rust-sqlx) and the [sqlx example project](../integrations/examples.md).

## aiosql and HugSQL query files

[aiosql](https://nackjicholson.github.io/aiosql/) (Python) and [HugSQL](https://www.hugsql.org) (Clojure) load named queries from `.sql` files. They are checked as they are, and diagnostics name the query they are in, as for sqlc:

```sql
-- name: get-user-by-id^
select id, nmae from users where id = :id;
```

```text
error[E0002]: Column 'nmae' not found in table 'users'
  --> queries/users.sql:2:12
    |
  2 | select id, nmae from users where id = :id;
    |            ^^^^
    = note: in query 'get-user-by-id'
    = help: Did you mean 'name'?
```

The name comments are recognised in every dialect:

- aiosql: `-- name: <name>` with an optional parameter list and operation suffix: `-- name: get-user-by-id^`, `-- name: list_posts(author)`, and `$`, `!`, `<!`, `*!`, `#`. The name is shown as written (`get-user-by-id`, not the Python method name `get_user_by_id`).
- HugSQL: `-- :name <name> :<command> :<result>` (`-- :name get-user :? :1`), `-- :name-` and `-- :snip <name>`.

A name comment starts a new statement, so queries need not end with `;`. In a file with such comments, parameters are untyped placeholders, like `$1`:

| Parameter | Becomes |
|---|---|
| aiosql `:id`, `:user.id`; HugSQL `:id`, `:user-id`, `:v:id`, `:v*:ids`, `:t:pair` | a placeholder (`($1)` right after `IN`) |
| HugSQL `VALUES :t*:rows` | rows of unknown columns, so the column count isn't checked |
| HugSQL `:i:col`, `:i*:cols`, `:identifier:tbl` | a name sqlsift doesn't know: it is never reported, and columns that may come from it aren't either |
| HugSQL `:sql:x`, `:snip:x`, `:snip*:x`, `:frag:x` | nothing; the query may have clauses and relations its text doesn't show, so names it can't resolve aren't reported |

HugSQL snippet bodies (after `-- :snip`) and queries with a Clojure expression (`--~ ...` or `/*~ ... ~*/`) are skipped: their SQL is only known at run time. `::` casts (`:id::int`), `:=`, array slices (`a[i:j]`) and anything in string literals, quoted identifiers and comments are left alone. Without name comments, `:name` is a psql variable with the PostgreSQL dialect (see [psql scripts](#psql-scripts)) and a bind parameter with MySQL and SQLite.

The [aiosql example](https://github.com/yukikotani231/sqlsift/tree/main/examples/aiosql) is a complete project.

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

## SQL in Rust (sqlx)

Rust files (`.rs`) are checked for the SQL passed to [sqlx](https://github.com/launchbadge/sqlx)'s query macros, the same SQL `cargo build` checks against a live database or the `.sqlx` cache, here against the migrations alone:

```rust
let posts = sqlx::query_as!(
    Post,
    r#"SELECT id, titel AS "title!" FROM posts WHERE author_id = $1"#,
    author_id
)
.fetch_all(&pool)
.await?;
```

```toml
schema_dir = "migrations"
files = ["src/**/*.rs"]
```

A string literal (plain, or raw: `r"..."`, `r#"..."#`) is checked when it is

- the first argument of `query!`, `query_as!`, `query_scalar!` or their `_unchecked` variants, or the second after a type (`query_as!(Post, "...")`), written with or without the `sqlx::` path;
- the first argument of the functions of the same names, when called through the `sqlx` crate: `sqlx::query("...")`, `sqlx::query_as::<_, Post>("...")`, `sqlx::query_scalar("...")`. These are checked by sqlx only when they run, so this is where sqlsift catches what the build doesn't. An unqualified `query("...")` is not checked, since any function may be named so.

A string that doesn't start with a statement keyword (`SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE`, ...) is skipped, so another crate's `query!` macro that takes something else is left alone.

`embedded_sql_macros` in `sqlsift.toml` replaces the list of macro and function names, for example to add your own macros wrapping sqlx's. Parameters are sqlx's own (`$1` for PostgreSQL, `?` for MySQL and SQLite), and type overrides in column aliases (`AS "id!"`, `AS "status: Status"`) are ordinary quoted aliases. `query_file!` needs nothing here: add its `.sql` files to `files` (see [sqlx query files](#sqlx-query-files)).

Diagnostics point at the query's line and column in the Rust file. Escapes in plain strings are read as Rust does (`\"name\"` is a quoted identifier). A glob pattern skips Rust files in `target` directories. SQL built at run time (`format!`, `QueryBuilder`) is not checked. [Suppression comments](suppression.md) work as Rust comments (`// sqlsift:disable E0002` on the line before a query, `// sqlsift:disable-file`), as in TypeScript.

`--stdin-filename` with an `.rs` extension checks stdin the same way.

## SQL in Python

Python files (`.py`) are checked for the SQL passed as a string literal to database calls: DB-API cursors (psycopg, sqlite3, MySQLdb, PyMySQL), asyncpg connections, SQLAlchemy's `text()` and Django's `raw()`:

```python
cur.execute(
    """
    SELECT id, titel
    FROM books
    WHERE author_id = %s
    """,
    (author_id,),
)
rows = await conn.fetch("SELECT id FROM books WHERE author_id = $1", author_id)
result = session.execute(text("SELECT id FROM books WHERE title = :title"), {"title": t})
```

```toml
schema_dir = "migrations"
files = ["app/**/*.py"]
```

Guessing which strings are SQL would report on strings that aren't, so a string is checked only when it is the first argument of a call whose name (the last part of the callee: `execute` matches `cur.execute(...)` and `self.db.execute(...)`) is in `embedded_sql_functions`: by default `execute`, `executemany`, `executescript`, `exec_driver_sql`, `fetch`, `fetchrow`, `fetchval`, `raw` and `text`. It must also start with a statement keyword (`SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE`, ...), so `execute("not sql")` in some other library is left alone.

- Plain, raw (`r"..."`) and triple-quoted strings are checked, and literals written side by side (`"SELECT a " "FROM t"`) are one query. Escapes are read as Python does.
- f-strings, byte strings and SQL built with `+`, `%` or `.format()` are skipped: their SQL is only known at run time.
- Driver placeholders are untyped parameters: `%s` and `%(name)s` (psycopg, MySQLdb, PyMySQL, Django), `:name` (SQLAlchemy `text()`, sqlite3), `?` (sqlite3) and `$1` (asyncpg). After `IN`, a placeholder is a list (`IN %s` with a tuple), and `%%` is a `%`.

Diagnostics point at the query's line and column in the Python file. A glob pattern skips Python files in virtual environments (`.venv`, `venv`, `site-packages`) and `__pycache__`. [Suppression comments](suppression.md) work as Python comments (`# sqlsift:disable E0002` on the line before a query, `# sqlsift:disable-file`). `--stdin-filename` with a `.py` extension checks stdin the same way. See the [psycopg example project](../integrations/examples.md).

## SQL in Go

Go files (`.go`) are checked for the SQL passed as a string literal to `database/sql`, [sqlx](https://jmoiron.github.io/sqlx/) and [pgx](https://github.com/jackc/pgx) calls:

```go
row := db.QueryRowContext(ctx, `
    SELECT id, titel
    FROM books
    WHERE author_id = $1`, authorID)
err := db.SelectContext(ctx, &books, "SELECT id, title FROM books WHERE price < ?", max)
_, err = pool.Exec(ctx, "UPDATE books SET in_stock = @in_stock WHERE id = @id", args)
```

```toml
schema_dir = "migrations"
files = ["internal/**/*.go"]
```

A string is checked only when it is an argument of a call whose name (the last part of the callee: `QueryContext` matches `db.QueryContext(...)` and `r.pool.QueryContext(...)`) is in `embedded_sql_go_functions`: by default `Query`, `QueryRow`, `Exec`, `Prepare` and their `Context` variants, sqlx's `Get`, `Select`, `Queryx`, `QueryRowx`, `MustExec`, `NamedExec`, `NamedQuery`, `Preparex` (and `Context` variants) and `sqlx.In`. For these, the string must be where the query goes: after the context, the destination (`Get(&user, ...)`) and the database (`sqlx.Get(db, &user, ...)`), so `db.Exec(query, "select")` never reads the argument as SQL. For names you add, any argument is checked. The string must also start with a statement keyword and have more than one word, so `c.Query("start")` in a web framework is left alone.

- Interpreted (`"..."`) and raw (`` `...` ``) strings are checked, and literals joined with `+` (`"SELECT a " +` and `"FROM t"` on the next line) are one query. Escapes are read as Go does.
- `fmt.Sprintf`, constants, variables and SQL joined with variables are skipped: their SQL is only known at run time. sqlc's generated code passes constants, so check sqlc's `.sql` query files instead.
- Driver placeholders are untyped parameters: `$1` (pgx, lib/pq), `?` (go-sql-driver/mysql, SQLite drivers, and sqlx queries passed through `db.Rebind`), `:name` (sqlx named queries) and `@name` (pgx named arguments). `IN (?)` after `sqlx.In` is a list. In PostgreSQL, the jsonb operators `?`, `?|` and `?&` stay operators.

Diagnostics point at the query's line and column in the Go file. A glob pattern skips Go files in `vendor`. [Suppression comments](suppression.md) work as Go comments (`// sqlsift:disable E0002` on the line before a query, `// sqlsift:disable-file`). `--stdin-filename` with a `.go` extension checks stdin the same way. See the [go-database-sql example project](../integrations/examples.md).

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | No errors (warnings may have been reported) |
| `1` | At least one error, or more warnings than `--max-warnings` / `max_warnings` |
| `2` | Usage or configuration error: missing files, a pattern that matches no files, an invalid `sqlsift.toml`, … |
