# sqlsift

[![CI](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/sqlsift-cli.svg)](https://www.npmjs.com/package/sqlsift-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Playground](https://img.shields.io/badge/try_it-playground-2ea44f.svg)](https://sqlsift.dev/)
[![Docs](https://img.shields.io/badge/docs-user_guide-0f766e.svg)](https://sqlsift.dev/docs/)

**Schema-aware static analysis for SQL. No database needed.**

Think of it as a type checker for your SQL: sqlsift replays your migrations into an in-memory schema and checks every query against it — in `.sql` files, sqlc and aiosql query files, `` sql`...` `` templates in TypeScript / JavaScript, sqlx queries in Rust, and SQL strings passed to database calls in Python and Go. It runs offline in milliseconds, so it fits in your editor, pre-commit hooks and CI.

**▶ [Try it in your browser](https://sqlsift.dev/)** — no install; the playground runs sqlsift locally via WebAssembly, so your SQL never leaves the page. **📖 [Read the user guide](https://sqlsift.dev/docs/)** for everything else.

<p align="center"><img src="https://raw.githubusercontent.com/yukikotani231/sqlsift/main/.github/assets/demo.gif" alt="A migration renames page_views.site_id to website_id; sqlsift init detects the project and sqlsift check reports the query that still uses site_id, an ON CONFLICT target with no unique key, and a WHERE clause that turns a LEFT JOIN into an inner join" width="900"></p>

<details>
<summary>The same output as text</summary>

```console
$ npx sqlsift-cli init     # finds your migrations, dialect and queries, writes sqlsift.toml
$ npx sqlsift-cli check
error[E0002]: Column 'site_id' not found in table 'page_views'
  --> src/retention.ts:6:11
    |
  6 |     WHERE site_id = ${siteId} AND created_at < ${cutoff}
    |           ^^^^^^^
    = help: 'site_id' was renamed to 'website_id' in migrations/002_rename_site_id.sql:1

error[E0014]: No unique constraint or primary key of 'users' matches ON CONFLICT (email)
  --> src/retention.ts:11:70
    |
 11 |   await sql`INSERT INTO users (email) VALUES (${email}) ON CONFLICT (email) DO NOTHING`;
    |                                                                      ^^^^^
    = help: The unique keys of 'users' are (id); add a unique constraint or index on (email)

warning[E0023]: WHERE condition on 'p.published_at' removes the rows the outer join adds without a match in 'p': it works as an inner join
  --> src/retention.ts:19:11
    |
 19 |     WHERE p.published_at > now() - interval '7 days'
    |           ^^^^^^^^^^^^^^
    = help: Move the condition into the join's ON clause to keep unmatched rows, or write an inner JOIN (or test 'p.published_at IS NULL OR ...')


Found 2 error(s), 1 warning(s) in 1 file(s)
```

</details>

[28 rules](#diagnostic-rules) cover what the schema alone can tell you: missing tables and columns, type mismatches, NOT NULL columns left out, `ON CONFLICT` targets with no unique key, `GROUP BY` errors, values out of range, and warnings for queries that are valid but probably not what you meant. Anything sqlsift can't work out is left unreported: on 16 open-source apps (687 files) it reports zero false positives.

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

Prebuilt binaries are also available on the [Releases](https://github.com/yukikotani231/sqlsift/releases) page, or try it without installing in the [Playground](https://sqlsift.dev/).

## Quick Start

In an existing project, start with:

```bash
sqlsift init
```

It finds your schema (sqlc config, Prisma / Supabase / Drizzle / Flyway / golang-migrate / goose / dbmate migrations, `structure.sql` or `schema.sql` dumps, dbt), your queries (`.sql` files, Prisma TypedSQL, dbt models, `` sql`...` `` templates in TypeScript / JavaScript, sqlx queries in Rust, database calls in Python and Go) and the dialect, writes a commented `sqlsift.toml`, runs a first check and prints the next steps. It never overwrites an existing `sqlsift.toml` without `--force`; `--dry-run` only prints the configuration. See [`sqlsift init`](https://sqlsift.dev/docs/reference/cli.html#sqlsift-init).

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

# SQL strings passed to database/sql, sqlx and pgx calls in Go
# (db.QueryContext(ctx, "..."); vendor is skipped; more calls with
# `embedded_sql_go_functions` in sqlsift.toml)
sqlsift check -s schema.sql 'internal/**/*.go'
```

To avoid repeating flags, add a `sqlsift.toml` to your project root:

```toml
schema_dir = "db/migrations"     # or: schema = ["db/schema.sql"]
files = ["queries/**/*.sql"]
dialect = "postgresql"

[rules]
ambiguous-column = "warn"        # report, but don't fail the check
```

Then just run `sqlsift check`. See the [configuration reference](https://sqlsift.dev/docs/reference/config.html) for every key, and `sqlsift schema` to [inspect what sqlsift loaded](https://sqlsift.dev/docs/guide/schema.html#inspecting-the-loaded-schema) from your schema.

## Use It With Your Stack

sqlsift only needs SQL files for the schema, so it works with whatever produces them.

| Stack | Schema source |
|-------|---------------|
| **Prisma** | `sqlsift check --schema-dir prisma/migrations queries/*.sql` |
| **Rails** (`schema_format = :sql`) | `sqlsift check --schema db/structure.sql queries/*.sql` |
| **sqlx / golang-migrate / goose / Flyway / dbmate** | `sqlsift check --schema-dir migrations queries/*.sql` |
| **`pg_dump --schema-only`** | `sqlsift check --schema schema.sql queries/*.sql` |
| **Hand-written DDL** | `sqlsift check --schema schema/*.sql queries/**/*.sql` |
| **aiosql / HugSQL** (named queries in `.sql` files) | `sqlsift check --schema-dir migrations queries/*.sql` (`:param` placeholders; diagnostics name the `-- name:` / `-- :name` query) |
| **dbt** (Postgres, MySQL, SQLite; Snowflake, BigQuery, Redshift, Databricks in alpha) | `sqlsift check --schema sources.sql models/**/*.sql` (Jinja is masked automatically next to `dbt_project.yml`; after `dbt docs generate`, `ref()` / `source()` columns come from `target/catalog.json` and `--schema` is optional — alpha) |

Rollback migrations are skipped automatically; see [Loading your schema](https://sqlsift.dev/docs/guide/schema.html).

**Example projects**, each with a ready-to-copy `sqlsift.toml` and checked in CI: [sqlc (Go)](examples/sqlc-go), [database/sql, sqlx and pgx (Go)](examples/go-database-sql), [Prisma TypedSQL](examples/prisma-typedsql), [aiosql (Python)](examples/aiosql), [postgres.js](examples/postgres-js), [sqlx (Rust)](examples/sqlx), [plain SQL + migrations](examples/postgres-migrations) (a migration renames a column that a query still uses), [MySQL](examples/mysql) and [dbt on PostgreSQL](examples/dbt-postgres). See [`examples/`](examples).

## Editor Integration

sqlsift ships a language server (`sqlsift-lsp`) that shows diagnostics as you type. Install the **sqlsift** VS Code extension (`sqlsift.sqlsift`), which bundles the server, or [set it up in Neovim, Helix and other editors](https://sqlsift.dev/docs/integrations/editors.html).

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

The action is also listed on the [GitHub Marketplace](https://github.com/marketplace/actions/sqlsift). Errors are shown as annotations on the pull request diff. Add `diff-base: ${{ github.base_ref }}` to report only the diagnostics a pull request introduces (the action checks the base commit too, so no baseline file is needed). The [CI guide](https://sqlsift.dev/docs/integrations/ci.html) covers the action's inputs, re-checking every query when the schema changes, GitHub Code Scanning (SARIF), `--format github` / `json` for other setups, `--max-warnings` for rolling out a rule gradually, and a baseline for adopting sqlsift on an existing codebase (`sqlsift check --write-baseline` records the current diagnostics in `sqlsift-baseline.json`; with `baseline = "sqlsift-baseline.json"` in `sqlsift.toml` only new ones are reported). There is also a [pre-commit recipe](https://sqlsift.dev/docs/integrations/pre-commit.html).

## Diagnostic Rules

| Code | Name | Description |
|------|------|-------------|
| [E0001](https://sqlsift.dev/docs/rules/E0001.html) | table-not-found | Referenced table does not exist in schema |
| [E0002](https://sqlsift.dev/docs/rules/E0002.html) | column-not-found | Referenced column does not exist in table |
| [E0003](https://sqlsift.dev/docs/rules/E0003.html) | type-mismatch | Type incompatibility in expressions (comparisons, arithmetic, INSERT/UPDATE values, enums) |
| [E0004](https://sqlsift.dev/docs/rules/E0004.html) | potential-null-violation | Potential NOT NULL violation (explicit NULL assignment) |
| [E0005](https://sqlsift.dev/docs/rules/E0005.html) | column-count-mismatch | Column count doesn't match (INSERT values, column alias lists, subqueries, row comparisons) |
| [E0006](https://sqlsift.dev/docs/rules/E0006.html) | ambiguous-column | Column reference is ambiguous across tables |
| [E0007](https://sqlsift.dev/docs/rules/E0007.html) | join-type-mismatch | JOIN condition compares incompatible types |
| [E0008](https://sqlsift.dev/docs/rules/E0008.html) | missing-required-column | INSERT omits a NOT NULL column that has no default |
| [E0009](https://sqlsift.dev/docs/rules/E0009.html) | duplicate-name | Table alias or CTE name given twice in one query |
| [E0010](https://sqlsift.dev/docs/rules/E0010.html) | duplicate-target-column | Column given twice in an INSERT column list or UPDATE SET |
| [E0011](https://sqlsift.dev/docs/rules/E0011.html) | position-out-of-range | ORDER BY / GROUP BY position is not in the select list |
| [E0012](https://sqlsift.dev/docs/rules/E0012.html) | misplaced-aggregate | Aggregate or window function where it is not allowed |
| [E0013](https://sqlsift.dev/docs/rules/E0013.html) | generated-column-write | INSERT or UPDATE gives a value to a generated column |
| [E0014](https://sqlsift.dev/docs/rules/E0014.html) | unmatched-conflict-target | ON CONFLICT columns match no unique constraint |
| [E0015](https://sqlsift.dev/docs/rules/E0015.html) | distinct-order-by | SELECT DISTINCT ordered by a column it doesn't select |
| [E0016](https://sqlsift.dev/docs/rules/E0016.html) | grouping-error | Column must appear in GROUP BY or be used in an aggregate |
| [E0017](https://sqlsift.dev/docs/rules/E0017.html) | value-out-of-range | Literal too long or out of range for the column type |
| [E0018](https://sqlsift.dev/docs/rules/E0018.html) | null-comparison | Comparison with NULL using = or <> is never true |
| [E0019](https://sqlsift.dev/docs/rules/E0019.html) | not-in-with-nulls | NOT IN over a nullable subquery column |
| [E0020](https://sqlsift.dev/docs/rules/E0020.html) | outer-column-in-subquery | IN subquery selects a column of the outer query |
| [E0021](https://sqlsift.dev/docs/rules/E0021.html) | missing-join-condition | Tables in FROM with no condition linking them |
| [E0022](https://sqlsift.dev/docs/rules/E0022.html) | constant-condition | Condition that is always or never true from the schema |
| [E0023](https://sqlsift.dev/docs/rules/E0023.html) | outer-join-filtered | WHERE condition turns an outer join into an inner join |
| [E0024](https://sqlsift.dev/docs/rules/E0024.html) | unfiltered-write | UPDATE or DELETE without WHERE |
| [E0025](https://sqlsift.dev/docs/rules/E0025.html) | insert-without-columns | INSERT without a column list |
| [E0026](https://sqlsift.dev/docs/rules/E0026.html) | select-star | SELECT * in a query's result columns |
| [E0027](https://sqlsift.dev/docs/rules/E0027.html) | wrong-argument-type | Function or operator given an argument type it doesn't take |
| [E0028](https://sqlsift.dev/docs/rules/E0028.html) | limit-without-order-by | LIMIT or OFFSET without ORDER BY |
| [E0029](https://sqlsift.dev/docs/rules/E0029.html) | implicit-conversion | Type mismatch MySQL or SQLite converts implicitly |
| [E0030](https://sqlsift.dev/docs/rules/E0030.html) | foreign-key-join-mismatch | JOIN condition doesn't follow the foreign key between the tables |
| [E0031](https://sqlsift.dev/docs/rules/E0031.html) | join-fan-out | Aggregate over rows that another one-to-many join multiplies |
| [E0032](https://sqlsift.dev/docs/rules/E0032.html) | integer-division | Division of integer columns truncates toward zero |
| [E0033](https://sqlsift.dev/docs/rules/E0033.html) | timestamp-timezone-mismatch | Comparing or assigning timestamp with timestamptz depends on session TimeZone |
| [E0034](https://sqlsift.dev/docs/rules/E0034.html) | case-enum-missing-labels | CASE over an enum column misses some labels and has no ELSE |
| [E0035](https://sqlsift.dev/docs/rules/E0035.html) | duplicate-insert-value | Duplicate primary key or unique values in an INSERT VALUES list |
| [E0036](https://sqlsift.dev/docs/rules/E0036.html) | illegal-mix-of-collations | Comparison or expression combines incompatible collations in MySQL |

E0001-E0017, E0027, E0035, and E0036 report queries the database rejects and are errors by default; E0018-E0023, E0029-E0031, and E0033 (`suspicious`) report valid queries that almost certainly don't do what was meant and are warnings by default; E0024-E0026 and E0028 (`restriction`) and E0032, E0034 (`pedantic`) are off until enabled (`-W restriction`, `-W pedantic`).

Every rule can be set to `off`, `warn` or `error` per project (`[rules]` in `sqlsift.toml`) or per run (`-A` / `-W` / `-D`), and silenced for a line or a file with `-- sqlsift:disable` / `-- sqlsift:disable-file` comments, or for a whole existing backlog with a baseline (`--write-baseline` / `--baseline`). See [Rules and levels](https://sqlsift.dev/docs/guide/rules.html) and [Suppressing diagnostics](https://sqlsift.dev/docs/guide/suppression.html).

## Documentation

The [user guide](https://sqlsift.dev/docs/) covers:

- [Loading your schema](https://sqlsift.dev/docs/guide/schema.html) and [checking queries](https://sqlsift.dev/docs/guide/queries.html) (stdin, ignore patterns, DDL and psql scripts in query files, dbt / Jinja templates, sqlc, aiosql and HugSQL query files with named parameters, SQL in TypeScript tagged templates, and SQL in Rust, Python and Go code)
- [Dialects and SQL support](https://sqlsift.dev/docs/guide/sql-support.html), including what type checking covers
- [Command line](https://sqlsift.dev/docs/reference/cli.html), [configuration file](https://sqlsift.dev/docs/reference/config.html) and [output formats](https://sqlsift.dev/docs/reference/output-formats.html) reference
- [Troubleshooting](https://sqlsift.dev/docs/guide/troubleshooting.html)

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
- [x] SQL embedded in application code (sqlc, aiosql and HugSQL query names and parameters, TypeScript tagged templates, sqlx in Rust, database calls in Python and Go)
- [x] `sqlsift init`, example projects per stack, and a GitHub Action that reports only what a pull request introduces
- [x] 28 rules, including `suspicious` warnings and opt-in `restriction` rules
- [x] Snowflake, BigQuery, Redshift and Databricks dialects and dbt `catalog.json` (alpha)

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
