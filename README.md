# sqlsift

[![CI](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/sqlsift-cli.svg)](https://www.npmjs.com/package/sqlsift-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Playground](https://img.shields.io/badge/try_it-playground-2ea44f.svg)](https://yukikotani231.github.io/sqlsift/)
[![Docs](https://img.shields.io/badge/docs-user_guide-0f766e.svg)](https://yukikotani231.github.io/sqlsift/docs/)

**Catch broken SQL before it reaches production — without a database.**

sqlsift reads your schema (`CREATE TABLE`, migrations, `structure.sql`, …) and checks your raw SQL queries against it: missing tables, typo'd columns, type mismatches, wrong `INSERT` arity, ambiguous columns. It runs offline in milliseconds, so it fits in pre-commit hooks, CI, and your editor.

**▶ [Try it in your browser](https://yukikotani231.github.io/sqlsift/)** — no install; the playground runs sqlsift locally via WebAssembly, so your SQL never leaves the page. **📖 [Read the user guide](https://yukikotani231.github.io/sqlsift/docs/)** for everything else.

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

> **Status:** early development (alpha). Diagnostics may change between versions. Feedback, bug reports and real-world SQL that sqlsift gets wrong are very welcome — please [open an issue](https://github.com/yukikotani231/sqlsift/issues).

## Why sqlsift?

Raw SQL is usually only checked when it runs. Rename a column in a migration and a query in some other file breaks silently until it hits staging — or production. sqlsift closes that gap:

- **No database needed** — no Docker, no connection string, no test fixtures. Just your `.sql` files.
- **Knows your schema** — understands `CREATE TABLE`, `ALTER TABLE`, views, enums and migration directories, and keeps track of what each query can see (CTEs, subqueries, `LATERAL`, aliases).
- **Helpful diagnostics** — source spans plus "did you mean" suggestions for typos.
- **Runs everywhere** — CLI, GitHub Actions / SARIF code scanning, and a language server with a VS Code extension.
- **Fast** — written in Rust; checks a typical project in milliseconds.
- **PostgreSQL, MySQL and SQLite** dialects, plus Snowflake, BigQuery, Redshift and Databricks in alpha.

### How it compares

| Tool | What it checks | Needs a running DB? |
|------|----------------|---------------------|
| **sqlsift** | Queries against your schema (tables, columns, types) | **No** |
| [SQLFluff](https://github.com/sqlfluff/sqlfluff) | Style and formatting | No — but does not know your schema |
| [Squawk](https://github.com/sbdchd/squawk) | Migration safety (locking, backwards compatibility) | No — lints DDL, not queries |
| [sqlx](https://github.com/launchbadge/sqlx) `query!` | Queries in Rust code, at compile time | Yes (or a cache prepared from one) |
| [sqlc](https://github.com/sqlc-dev/sqlc) | Queries it generates code from | No — but you adopt its codegen workflow |

sqlsift complements these tools: keep your formatter and migration linter, and add sqlsift to make sure the queries still match the schema.

## Installation

```bash
# npm (prebuilt binaries for macOS, Linux and Windows)
npm install -g sqlsift-cli
# or run without installing
npx sqlsift-cli check --schema schema.sql queries/*.sql

# From source (Rust toolchain required)
cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-cli
```

Prebuilt binaries are also available on the [Releases](https://github.com/yukikotani231/sqlsift/releases) page, or try it without installing in the [Playground](https://yukikotani231.github.io/sqlsift/).

## Quick Start

In an existing project, start with:

```bash
sqlsift init
```

It finds your schema (sqlc config, Prisma / Supabase / Drizzle / Flyway / golang-migrate / goose / dbmate migrations, `structure.sql` or `schema.sql` dumps, dbt), your queries (`.sql` files, Prisma TypedSQL, dbt models, `` sql`...` `` templates in TypeScript / JavaScript) and the dialect, writes a commented `sqlsift.toml`, runs a first check and prints the next steps. It never overwrites an existing `sqlsift.toml` without `--force`; `--dry-run` only prints the configuration. See [`sqlsift init`](https://yukikotani231.github.io/sqlsift/docs/reference/cli.html#sqlsift-init).

Or pass everything on the command line:

```bash
# Validate queries against a schema file
sqlsift check --schema schema.sql queries/*.sql

# Use multiple schema files
sqlsift check -s users.sql -s orders.sql queries/*.sql

# Use a migrations directory (all *.sql files, recursively, in filename order;
# rollback files such as *.down.sql are skipped)
sqlsift check --schema-dir ./migrations queries/*.sql

# Other dialects
sqlsift check --dialect mysql --schema schema.sql queries/*.sql

# Read a query from stdin (e.g. the staged version in a pre-commit hook)
git show :queries/users.sql | sqlsift check -s schema.sql --stdin-filename queries/users.sql -

# SQL in TypeScript / JavaScript (and Vue / Svelte <script>) tagged templates
# (sql`...`; node_modules and dist are skipped; more tags with
# `embedded_sql_tags` in sqlsift.toml)
sqlsift check -s schema.sql 'src/**/*.ts'
```

To avoid repeating flags, add a `sqlsift.toml` to your project root:

```toml
schema_dir = "db/migrations"     # or: schema = ["db/schema.sql"]
files = ["queries/**/*.sql"]
dialect = "postgresql"

[rules]
ambiguous-column = "warn"        # report, but don't fail the check
```

Then just run `sqlsift check`. See the [configuration reference](https://yukikotani231.github.io/sqlsift/docs/reference/config.html) for every key, and `sqlsift schema` to [inspect what sqlsift loaded](https://yukikotani231.github.io/sqlsift/docs/guide/schema.html#inspecting-the-loaded-schema) from your schema.

## Use It With Your Stack

sqlsift only needs SQL files for the schema, so it works with whatever produces them.

| Stack | Schema source |
|-------|---------------|
| **Prisma** | `sqlsift check --schema-dir prisma/migrations queries/*.sql` |
| **Rails** (`schema_format = :sql`) | `sqlsift check --schema db/structure.sql queries/*.sql` |
| **sqlx / golang-migrate / goose / Flyway / dbmate** | `sqlsift check --schema-dir migrations queries/*.sql` |
| **`pg_dump --schema-only`** | `sqlsift check --schema schema.sql queries/*.sql` |
| **Hand-written DDL** | `sqlsift check --schema schema/*.sql queries/**/*.sql` |
| **dbt** (Postgres, MySQL, SQLite; Snowflake, BigQuery, Redshift, Databricks in alpha) | `sqlsift check --schema sources.sql models/**/*.sql` (Jinja is masked automatically next to `dbt_project.yml`; after `dbt docs generate`, `ref()` / `source()` columns come from `target/catalog.json` and `--schema` is optional — alpha) |

Rollback migrations are skipped automatically; see [Loading your schema](https://yukikotani231.github.io/sqlsift/docs/guide/schema.html).

**Example projects**, each with a ready-to-copy `sqlsift.toml` and checked in CI: [sqlc (Go)](examples/sqlc-go), [Prisma TypedSQL](examples/prisma-typedsql), [postgres.js](examples/postgres-js), [sqlx (Rust)](examples/sqlx), [plain SQL + migrations](examples/postgres-migrations) (a migration renames a column that a query still uses), [MySQL](examples/mysql) and [dbt on PostgreSQL](examples/dbt-postgres). See [`examples/`](examples).

## Editor Integration

sqlsift ships a language server (`sqlsift-lsp`) that shows diagnostics as you type. Install the **sqlsift** VS Code extension (`sqlsift.sqlsift`), which bundles the server, or [set it up in Neovim, Helix and other editors](https://yukikotani231.github.io/sqlsift/docs/integrations/editors.html).

## CI Integration

```yaml
# .github/workflows/sqlsift.yml
name: SQL Lint
on: [push, pull_request]
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: yukikotani231/sqlsift@main  # or pin a release tag
        with:
          schema: db/schema.sql           # or schema-dir: db/migrations
          files: queries/**/*.sql
```

Errors are shown as annotations on the pull request diff. Add `diff-base: ${{ github.base_ref }}` to report only the diagnostics a pull request introduces (the action checks the base commit too, so no baseline file is needed). The [CI guide](https://yukikotani231.github.io/sqlsift/docs/integrations/ci.html) covers the action's inputs, re-checking every query when the schema changes, GitHub Code Scanning (SARIF), `--format github` / `json` for other setups, `--max-warnings` for rolling out a rule gradually, and a baseline for adopting sqlsift on an existing codebase (`sqlsift check --write-baseline` records the current diagnostics in `sqlsift-baseline.json`; with `baseline = "sqlsift-baseline.json"` in `sqlsift.toml` only new ones are reported). There is also a [pre-commit recipe](https://yukikotani231.github.io/sqlsift/docs/integrations/pre-commit.html).

## Diagnostic Rules

| Code | Name | Description |
|------|------|-------------|
| [E0001](https://yukikotani231.github.io/sqlsift/docs/rules/E0001.html) | table-not-found | Referenced table does not exist in schema |
| [E0002](https://yukikotani231.github.io/sqlsift/docs/rules/E0002.html) | column-not-found | Referenced column does not exist in table |
| [E0003](https://yukikotani231.github.io/sqlsift/docs/rules/E0003.html) | type-mismatch | Type incompatibility in expressions (comparisons, arithmetic, INSERT/UPDATE values, enums) |
| [E0004](https://yukikotani231.github.io/sqlsift/docs/rules/E0004.html) | potential-null-violation | Potential NOT NULL violation (explicit NULL assignment) |
| [E0005](https://yukikotani231.github.io/sqlsift/docs/rules/E0005.html) | column-count-mismatch | Column count doesn't match (INSERT values, column alias lists, subqueries, row comparisons) |
| [E0006](https://yukikotani231.github.io/sqlsift/docs/rules/E0006.html) | ambiguous-column | Column reference is ambiguous across tables |
| [E0007](https://yukikotani231.github.io/sqlsift/docs/rules/E0007.html) | join-type-mismatch | JOIN condition compares incompatible types |
| [E0008](https://yukikotani231.github.io/sqlsift/docs/rules/E0008.html) | missing-required-column | INSERT omits a NOT NULL column that has no default |
| [E0009](https://yukikotani231.github.io/sqlsift/docs/rules/E0009.html) | duplicate-name | Table alias or CTE name given twice in one query |
| [E0010](https://yukikotani231.github.io/sqlsift/docs/rules/E0010.html) | duplicate-target-column | Column given twice in an INSERT column list or UPDATE SET |
| [E0011](https://yukikotani231.github.io/sqlsift/docs/rules/E0011.html) | position-out-of-range | ORDER BY / GROUP BY position is not in the select list |
| [E0012](https://yukikotani231.github.io/sqlsift/docs/rules/E0012.html) | misplaced-aggregate | Aggregate or window function where it is not allowed |
| [E0013](https://yukikotani231.github.io/sqlsift/docs/rules/E0013.html) | generated-column-write | INSERT or UPDATE gives a value to a generated column |
| [E0014](https://yukikotani231.github.io/sqlsift/docs/rules/E0014.html) | unmatched-conflict-target | ON CONFLICT columns match no unique constraint |
| [E0015](https://yukikotani231.github.io/sqlsift/docs/rules/E0015.html) | distinct-order-by | SELECT DISTINCT ordered by a column it doesn't select |
| [E0016](https://yukikotani231.github.io/sqlsift/docs/rules/E0016.html) | grouping-error | Column must appear in GROUP BY or be used in an aggregate |
| [E0017](https://yukikotani231.github.io/sqlsift/docs/rules/E0017.html) | value-out-of-range | Literal too long or out of range for the column type |
| [E0018](https://yukikotani231.github.io/sqlsift/docs/rules/E0018.html) | null-comparison | Comparison with NULL using = or <> is never true |
| [E0019](https://yukikotani231.github.io/sqlsift/docs/rules/E0019.html) | not-in-with-nulls | NOT IN over a nullable subquery column |
| [E0020](https://yukikotani231.github.io/sqlsift/docs/rules/E0020.html) | outer-column-in-subquery | IN subquery selects a column of the outer query |
| [E0021](https://yukikotani231.github.io/sqlsift/docs/rules/E0021.html) | missing-join-condition | Tables in FROM with no condition linking them |
| [E0022](https://yukikotani231.github.io/sqlsift/docs/rules/E0022.html) | constant-condition | Condition that is always or never true from the schema |
| [E0023](https://yukikotani231.github.io/sqlsift/docs/rules/E0023.html) | outer-join-filtered | WHERE condition turns an outer join into an inner join |
| [E0024](https://yukikotani231.github.io/sqlsift/docs/rules/E0024.html) | unfiltered-write | UPDATE or DELETE without WHERE |
| [E0025](https://yukikotani231.github.io/sqlsift/docs/rules/E0025.html) | insert-without-columns | INSERT without a column list |
| [E0026](https://yukikotani231.github.io/sqlsift/docs/rules/E0026.html) | select-star | SELECT * in a query's result columns |
| [E0027](https://yukikotani231.github.io/sqlsift/docs/rules/E0027.html) | wrong-argument-type | Function or operator given an argument type it doesn't take |

E0001-E0017 and E0027 report queries the database rejects and are errors by default; E0018-E0023 (`suspicious`) report valid queries that almost certainly don't do what was meant and are warnings by default; E0024-E0026 (`restriction`) are off until enabled (`-W restriction`).

Every rule can be set to `off`, `warn` or `error` per project (`[rules]` in `sqlsift.toml`) or per run (`-A` / `-W` / `-D`), and silenced for a line or a file with `-- sqlsift:disable` / `-- sqlsift:disable-file` comments, or for a whole existing backlog with a baseline (`--write-baseline` / `--baseline`). See [Rules and levels](https://yukikotani231.github.io/sqlsift/docs/guide/rules.html) and [Suppressing diagnostics](https://yukikotani231.github.io/sqlsift/docs/guide/suppression.html).

## Documentation

The [user guide](https://yukikotani231.github.io/sqlsift/docs/) covers:

- [Loading your schema](https://yukikotani231.github.io/sqlsift/docs/guide/schema.html) and [checking queries](https://yukikotani231.github.io/sqlsift/docs/guide/queries.html) (stdin, ignore patterns, DDL and psql scripts in query files, dbt / Jinja templates, sqlc query files with named parameters, and SQL in TypeScript tagged templates)
- [Dialects and SQL support](https://yukikotani231.github.io/sqlsift/docs/guide/sql-support.html), including what type checking covers
- [Command line](https://yukikotani231.github.io/sqlsift/docs/reference/cli.html), [configuration file](https://yukikotani231.github.io/sqlsift/docs/reference/config.html) and [output formats](https://yukikotani231.github.io/sqlsift/docs/reference/output-formats.html) reference
- [Troubleshooting](https://yukikotani231.github.io/sqlsift/docs/guide/troubleshooting.html)

## Roadmap

#### Completed
- [x] Configuration file (`sqlsift.toml`)
- [x] MySQL dialect support
- [x] SQLite dialect support
- [x] Type inference for expressions (WHERE, JOIN, arithmetic, INSERT/UPDATE)
- [x] LSP server for editor integration (VS Code extension)
- [x] CASE expression type consistency checking
- [x] Subquery/CTE/VIEW column type inference

- [x] Per-rule and per-category levels (`off` / `warn` / `error`)
- [x] Baseline of known diagnostics (`--write-baseline`, `baseline` in `sqlsift.toml`)
- [x] SQL embedded in application code (sqlc query names and parameters, TypeScript tagged templates)

#### Planned
See [ROADMAP.md](ROADMAP.md) for what comes next.

## Contributing

Contributions are welcome! The most useful things right now are:

- **Real-world SQL that sqlsift gets wrong** — false positives and missed errors. Please [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with a minimal schema and query.
- **Dialect coverage** — MySQL and SQLite edge cases, and the alpha warehouse dialects (Snowflake, BigQuery, Redshift, Databricks).
- **Editor integrations** — setup guides or plugins for editors other than VS Code.

To work on the code:

```bash
git clone https://github.com/yukikotani231/sqlsift && cd sqlsift
git config core.hooksPath .githooks   # fmt + clippy pre-commit checks
cargo test
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/invalid_query.sql
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the development setup, repository layout and how to add a rule. Commits follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:` …), which drive automated releases.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs) — SQL parsing
- [miette](https://github.com/zkat/miette) — Diagnostic rendering
- [clap](https://github.com/clap-rs/clap) — CLI framework
