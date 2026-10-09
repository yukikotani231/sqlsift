# Introduction

**sqlsift catches broken SQL before it reaches production, without a database.**

sqlsift reads your schema (`CREATE TABLE` files, migrations, `structure.sql`, a `pg_dump --schema-only` dump, …) and checks your raw SQL queries against it: missing tables, typo'd columns, type mismatches, wrong `INSERT` arity, ambiguous columns. It runs offline in milliseconds, so it fits in pre-commit hooks, CI and your editor.

```sql
-- schema.sql
CREATE TABLE users  (id SERIAL PRIMARY KEY, name VARCHAR(100) NOT NULL, email TEXT UNIQUE);
CREATE TABLE orders (id SERIAL PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id), total NUMERIC(10, 2));
```

```sql
-- queries/report.sql
SELECT u.naem, o.total
FROM users u
JOIN orders o ON o.user_id = u.email
WHERE u.id = 'abc';
```

```console
$ npx sqlsift-cli check --schema schema.sql queries/report.sql
error[E0002]: Column 'naem' not found in table 'users'
  --> queries/report.sql:1:10
    |
  1 | SELECT u.naem, o.total
    |          ^^^^
    = help: Did you mean 'name'?

error[E0007]: JOIN condition type mismatch: integer vs text
  --> queries/report.sql:3:18
    |
  3 | JOIN orders o ON o.user_id = u.email
    |                  ^^^^^^^^^
    = help: JOIN condition should compare compatible types. Consider using explicit CAST.

error[E0003]: Type mismatch: cannot compare integer with text
  --> queries/report.sql:4:7
    |
  4 | WHERE u.id = 'abc';
    |       ^^^^
    = help: Types are not implicitly compatible. Consider using explicit CAST.


Found 3 error(s), 0 warning(s) in 1 file(s)
```

Want to see it first? The [playground](https://yukikotani231.github.io/sqlsift/) runs sqlsift in your browser via WebAssembly; your SQL never leaves the page.

## Why sqlsift?

Raw SQL is usually only checked when it runs. Rename a column in a migration and a query in some other file breaks silently until it hits staging, or production. sqlsift closes that gap:

- **No database needed.** No Docker, no connection string, no test fixtures. Just your `.sql` files.
- **Knows your schema.** It understands `CREATE TABLE`, `ALTER TABLE`, views, enums and migration directories, and keeps track of what each query can see (CTEs, subqueries, `LATERAL`, aliases).
- **Helpful diagnostics.** Source spans plus "did you mean" suggestions for typos.
- **Runs everywhere.** CLI, GitHub Actions and SARIF code scanning, and a language server with a VS Code extension.
- **Fast.** Written in Rust; checks a typical project in milliseconds.
- **PostgreSQL, MySQL and SQLite** dialects.

## How it compares

| Tool | What it checks | Needs a running DB? |
|------|----------------|---------------------|
| **sqlsift** | Queries against your schema (tables, columns, types) | **No** |
| [SQLFluff](https://github.com/sqlfluff/sqlfluff) | Style and formatting | No, but it does not know your schema |
| [Squawk](https://github.com/sbdchd/squawk) | Migration safety (locking, backwards compatibility) | No; it lints DDL, not queries |
| [sqlx](https://github.com/launchbadge/sqlx) `query!` | Queries in Rust code, at compile time | Yes (or a cache prepared from one) |
| [sqlc](https://github.com/sqlc-dev/sqlc) | Queries it generates code from | No, but you adopt its codegen workflow |

sqlsift complements these tools: keep your formatter and migration linter, and add sqlsift to make sure the queries still match the schema.

## Status

sqlsift is in early development (alpha). Diagnostics may change between versions. Feedback, bug reports and real-world SQL that sqlsift gets wrong are very welcome: please [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose).
