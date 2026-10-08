# sqlsift

[![CI](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/sqlsift-cli.svg)](https://www.npmjs.com/package/sqlsift-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Playground](https://img.shields.io/badge/try_it-playground-2ea44f.svg)](https://yukikotani231.github.io/sqlsift/)

**Catch broken SQL before it reaches production — without a database.**

sqlsift reads your schema (`CREATE TABLE`, migrations, `structure.sql`, …) and checks your raw SQL queries against it: missing tables, typo'd columns, type mismatches, wrong `INSERT` arity, ambiguous columns. It runs offline in milliseconds, so it fits in pre-commit hooks, CI, and your editor.

**▶ [Try it in your browser](https://yukikotani231.github.io/sqlsift/)** — no install; the playground runs sqlsift locally via WebAssembly, so your SQL never leaves the page.

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
- **PostgreSQL, MySQL and SQLite** dialects.

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

```bash
# Validate queries against a schema file
sqlsift check --schema schema.sql queries/*.sql

# Use multiple schema files
sqlsift check -s users.sql -s orders.sql queries/*.sql

# Use a migrations directory (all *.sql files, recursively, in filename order)
sqlsift check --schema-dir ./migrations queries/*.sql

# Other dialects
sqlsift check --dialect mysql --schema schema.sql queries/*.sql
```

To avoid repeating flags, add a `sqlsift.toml` to your project root (see [`sqlsift.toml`](sqlsift.toml) for all options):

```toml
schema = ["db/schema.sql"]
# schema_dir = "db/migrations"
# dialect = "postgresql"

# [rules]
# ambiguous-column = "warn"   # report, but don't fail the check
```

Then just run `sqlsift check queries/**/*.sql`.

### Inspecting the loaded schema

When a query is flagged unexpectedly, check what sqlsift actually understood from your schema. `sqlsift schema` takes the same schema options as `check` (`--schema`, `--schema-dir`, `--config`, `--dialect`) and falls back to `sqlsift.toml`:

```console
$ sqlsift schema --schema-dir migrations
Schema Information:
==================
Dialect: postgresql
Schema files:
  migrations/001_init.sql

Schema: public
  Table: users
    - id integer NOT NULL PRIMARY KEY DEFAULT nextval('users_id_seq'::regclass)
    - name text NOT NULL
    - feeling mood NULL
  View: user_names
    - id integer
    - name text
  Materialized view: user_count
    - n bigint

Enum types:
  mood: 'sad', 'ok', 'happy'
```

Objects are listed per schema in definition order. Statements sqlsift had to skip are reported as warnings on stderr.

`sqlsift schema --format json` prints the same information as JSON:

```jsonc
{
  "dialect": "postgresql",
  "default_schema": "public",
  "schema_files": ["migrations/001_init.sql"],
  "schemas": [{
    "name": "public",
    "tables": [{
      "name": "users",
      "columns": [{ "name": "id", "type": "integer", "nullable": false, "primary_key": true,
                    "identity": null, "auto_increment": false, "default": "nextval(...)" }],
      "primary_key": ["id"],           // or null
      "foreign_keys": [{ "name": null, "columns": ["..."], "references_table": "...", "references_columns": ["..."] }],
      "unique": [["..."]]
    }],
    "views": [{ "name": "user_names", "materialized": false,
                "columns": [{ "name": "id", "type": "integer" }] }]   // type is null when unknown
  }],
  "enums": [{ "name": "mood", "values": ["sad", "ok", "happy"] }]
}
```

<details>
<summary><b>Configuration file reference</b></summary>

`sqlsift check` and `sqlsift schema` look for `sqlsift.toml` in the current directory and its parents (or uses `--config <FILE>`). Command-line options override values from the file.

```toml
schema = ["db/schema/*.sql"]      # schema files (glob patterns supported)
# schema_dir = "db/migrations"    # all .sql files under this directory, in filename order
files = ["queries/**/*.sql"]      # query files to check (glob patterns supported)
dialect = "postgresql"            # postgresql, mysql or sqlite
format = "human"                  # human, json or sarif
disable = ["E0006"]               # rules to turn off (same as `E0006 = "off"` below)

[rules]                           # per-rule level: "off", "warn" or "error"
E0008 = "warn"                    # by code...
ambiguous-column = "off"          # ...or by name

[categories]                      # level of every rule in a category
correctness = "error"
```

Relative paths in the file are resolved against the directory containing `sqlsift.toml`. Unknown keys produce a warning; invalid `dialect` or `format` values, unknown rules and invalid levels are errors.

Exit codes: `0` when no errors were found, `1` when diagnostics with error severity were reported, `2` for usage or configuration errors (missing files, invalid config, etc.).

</details>

## Use It With Your Stack

sqlsift only needs SQL files for the schema, so it works with whatever produces them.

| Stack | Schema source |
|-------|---------------|
| **Prisma** | `sqlsift check --schema-dir prisma/migrations queries/*.sql` |
| **Rails** (`schema_format = :sql`) | `sqlsift check --schema db/structure.sql queries/*.sql` |
| **sqlx / golang-migrate / Flyway / dbmate** | `sqlsift check --schema-dir migrations queries/*.sql` |
| **`pg_dump --schema-only`** | `sqlsift check --schema schema.sql queries/*.sql` |
| **Hand-written DDL** | `sqlsift check --schema schema/*.sql queries/**/*.sql` |

## Editor Integration

sqlsift ships a language server (`sqlsift-lsp`) that shows diagnostics as you type.

- **VS Code** — install the **sqlsift** extension (`sqlsift.sqlsift`). Platform builds bundle the language server, so no extra setup is required. See [`editors/vscode`](editors/vscode) for details.
- **Other editors** (Neovim, Helix, Zed, …) — install the server with `cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-lsp` and register `sqlsift-lsp` as a language server for SQL files. It reads `sqlsift.toml` from the workspace root.

## CI Integration

### GitHub Action

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

Errors are shown as annotations on the pull request diff. All inputs are optional when you have a `sqlsift.toml`:

| Input | Description |
|-------|-------------|
| `files` | Query files (space-separated paths or globs) |
| `schema` / `schema-dir` | Schema files, or a directory of migrations |
| `dialect` | `postgresql`, `mysql` or `sqlite` |
| `config` | Path to `sqlsift.toml` |
| `disable` | Rules to disable, e.g. `E0006 E0008` |
| `sarif-file` | Also write a SARIF report (see below) |
| `fail-on-error` | Fail the step on errors (default `true`) |
| `version` | `sqlsift-cli` version from npm (default `latest`) |

The `exit-code` output is `0` (clean), `1` (errors found) or `2` (configuration error).

Prefer plain commands? `npx sqlsift-cli check --schema schema.sql queries/*.sql` works in any CI.

### GitHub Code Scanning (SARIF)

Show errors in the Security tab and as code scanning alerts:

```yaml
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    permissions:
      security-events: write
    steps:
      - uses: actions/checkout@v4
      - uses: yukikotani231/sqlsift@main
        with:
          schema: db/schema.sql
          files: queries/**/*.sql
          sarif-file: results.sarif
          fail-on-error: "false"
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: results.sarif
```

### JSON output

`--format json` writes a single JSON document to stdout (logs and the summary go to stderr). Only files with diagnostics are listed; `files` is empty when everything passes.

```json
{
  "files": [
    {
      "file": "queries/fetch.sql",
      "diagnostics": [
        {
          "code": "E0002",
          "kind": "ColumnNotFound",
          "severity": "error",
          "message": "Column 'user_id' not found",
          "help": "Did you mean 'id'?",
          "line": 3,
          "column": 15,
          "span": { "line": 3, "column": 15, "length": 7, "offset": 0 },
          "labels": []
        }
      ]
    }
  ]
}
```

The SARIF 2.1.0 log (`--format sarif`) contains a single run with results for all files and a `tool.driver.rules` entry for every diagnostic rule. Human output uses colors only when stderr is a terminal and `NO_COLOR` is not set.

## Diagnostic Rules

| Code | Name | Category | Description |
|------|------|----------|-------------|
| E0001 | table-not-found | correctness | Referenced table does not exist in schema |
| E0002 | column-not-found | correctness | Referenced column does not exist in table |
| E0003 | type-mismatch | correctness | Type incompatibility in expressions (comparisons, arithmetic) |
| E0004 | potential-null-violation | correctness | Potential NOT NULL violation (explicit NULL assignment) |
| E0005 | column-count-mismatch | correctness | INSERT column count doesn't match values |
| E0006 | ambiguous-column | correctness | Column reference is ambiguous across tables |
| E0007 | join-type-mismatch | correctness | JOIN condition compares incompatible types |
| E0008 | missing-required-column | correctness | INSERT omits a NOT NULL column that has no default |

`sqlsift rules` prints this list. Like [oxlint](https://oxc.rs/docs/guide/usage/linter.html), every rule belongs to a category that sets its default level:

| Category | Default | Meaning |
|----------|---------|---------|
| `correctness` | error | The query fails or does something unintended |
| `suspicious` | warn | The query is most likely wrong |
| `pedantic` | off | Stricter checks that may have false positives |
| `style` | off | Conventions and readability |
| `restriction` | off | Bans on features some codebases don't want |

Set the level of a rule or a whole category to `off`, `warn` or `error` with `[rules]` / `[categories]` in `sqlsift.toml`, or with `-A` (allow), `-W` (warn) and `-D` (deny) on the command line, using a rule's code or name or a category name. A rule's own level wins over its category's; command line flags win over the config file. Warnings are reported but don't fail `sqlsift check`.

```bash
sqlsift check -W ambiguous-column -A E0008 queries/*.sql
```

### Inline Suppression

Suppress diagnostics on specific lines using SQL comments:

```sql
-- Suppress a specific rule on the next line
-- sqlsift:disable E0002
SELECT legacy_col FROM users;

-- Suppress on the same line
SELECT legacy_col FROM users; -- sqlsift:disable E0002

-- Suppress multiple rules (codes or names)
SELECT bad_col FROM missing_table; -- sqlsift:disable E0001, column-not-found

-- Suppress all rules on the next line
-- sqlsift:disable
SELECT bad_col FROM missing_table;
```

<details>
<summary><b>Type inference coverage (E0003, E0007)</b></summary>

**Currently Detected:**
- ✅ WHERE clause comparisons (`WHERE id = 'text'`)
- ✅ Arithmetic operations (`SELECT name + 10`)
- ✅ JOIN conditions (`ON users.id = orders.user_name`)
- ✅ Set operations column validation (`UNION` / `INTERSECT` / `EXCEPT` column count and type compatibility)
- ✅ Potential NOT NULL violation checks for explicit `NULL` assignment in `INSERT` / `UPDATE` (`E0004`)
- ✅ INSERT value type mismatches (`INSERT INTO users (id) VALUES ('text')`)
- ✅ UPDATE assignment type mismatches (`UPDATE users SET id = 'text'`)
- ✅ CAST expression type inference (`CAST(name AS INTEGER)`)
- ✅ Function return type inference (e.g., `COUNT`, `SUM`, `UPPER`, `LENGTH`, `COALESCE`)
- ✅ Nested expressions (`WHERE (a + b) * 2 = 'text'`)
- ✅ All comparison operators (=, !=, <, >, <=, >=)
- ✅ Numeric type compatibility (INTEGER, BIGINT, DECIMAL, etc.)
- ✅ String literals coerce to the column type like in the database (`created_at > '2024-01-01'`, `status = 'active'`, `id = '42'`), while impossible values are still reported (`id = 'abc'`)
- ✅ Date/time arithmetic (`now() - interval '7 days'`, `placed_on + 7`, `ts1 - ts2`)
- ✅ CASE expression branch consistency (`THEN total ELSE 'cheap'`) and result type
- ✅ Enum values for PostgreSQL enum types and MySQL inline `ENUM(...)` (`status = 'opne'` → "Did you mean 'open'?")
- ✅ Column types through CTEs, subqueries, views and `CREATE TABLE ... AS` (`WITH t AS (SELECT id FROM users) SELECT * FROM t WHERE id = 'abc'`)

</details>

<details>
<summary><b>Supported SQL</b></summary>

### Queries

- SELECT, INSERT, UPDATE, DELETE with full column/table validation
- JOINs (INNER, LEFT, RIGHT, FULL, CROSS, NATURAL) with ON/USING clause validation
- CTEs (WITH clause) including recursive CTEs
- Subqueries (WHERE IN/EXISTS, FROM derived tables, scalar subqueries)
- LATERAL vs non-LATERAL scope isolation
- UPDATE ... FROM / DELETE ... USING (PostgreSQL extensions)
- Window functions (OVER, PARTITION BY, FILTER)
- GROUPING SETS, CUBE, ROLLUP
- DISTINCT ON, UNION / INTERSECT / EXCEPT
- ORDER BY with SELECT alias support
- Comprehensive expression coverage (CASE, CAST, JSON operators, AT TIME ZONE, ARRAY, etc.)

### DDL

- `CREATE TABLE` (columns, constraints, primary keys, foreign keys, UNIQUE)
- `CREATE VIEW` (column inference from SELECT projection)
- `CREATE TYPE AS ENUM`
- `ALTER TABLE` (ADD/DROP/RENAME COLUMN, ADD CONSTRAINT, RENAME TABLE)
- `CHECK` constraints (column-level and table-level)
- `GENERATED AS IDENTITY` columns (ALWAYS / BY DEFAULT)
- Resilient parsing — unsupported DDL (functions, triggers, domains, etc.) is gracefully skipped

### Dialects

- **PostgreSQL** (default) — fully supported
- **MySQL** — supported (`--dialect mysql`)
- **SQLite** — supported (`--dialect sqlite`)

Use the `--dialect` flag to specify the dialect.

</details>

<details>
<summary><b>CLI reference</b></summary>

```
sqlsift check [OPTIONS] <FILES>...

Arguments:
  <FILES>...                SQL files to validate (supports glob patterns)

Options:
  -s, --schema <FILE>       Schema definition file (can be specified multiple times)
      --schema-dir <DIR>    Directory containing schema files
  -c, --config <FILE>       Path to configuration file [default: sqlsift.toml]
  -A, --allow <RULE>        Turn a rule or category off (alias: --disable)
  -W, --warn <RULE>         Report a rule or category as warnings
  -D, --deny <RULE>         Report a rule or category as errors
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite [default: postgresql]
  -f, --format <FORMAT>     Output format: human, json, sarif [default: human]
      --max-errors <N>      Maximum number of errors before stopping [default: 100, 0 = unlimited]
  -v, --verbose             Enable verbose logging to stderr (-vv for debug)
  -q, --quiet               Suppress summary/non-error output
  -h, --help                Print help
```

```
sqlsift schema [OPTIONS] [FILES]...

Arguments:
  [FILES]...                Schema definition files (same as --schema)

Options:
  -s, --schema <FILE>       Schema definition file (can be specified multiple times)
      --schema-dir <DIR>    Directory containing schema files
  -c, --config <FILE>       Path to configuration file [default: sqlsift.toml]
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite [default: postgresql]
  -f, --format <FORMAT>     Output format: human, json [default: human]
```

`sqlsift rules` lists every rule with its category and default level.

</details>

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

#### Planned
- [ ] Custom rule plugins

## Contributing

Contributions are welcome! The most useful things right now are:

- **Real-world SQL that sqlsift gets wrong** — false positives and missed errors. Please [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with a minimal schema and query.
- **Dialect coverage** — MySQL and SQLite edge cases.
- **Editor integrations** — setup guides or plugins for editors other than VS Code.

To work on the code:

```bash
git clone https://github.com/yukikotani231/sqlsift && cd sqlsift
git config core.hooksPath .githooks   # fmt + clippy pre-commit checks
cargo test
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/invalid_query.sql
```

The architecture overview lives in [`CLAUDE.md`](CLAUDE.md). Commits follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:` …), which drive automated releases.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs) — SQL parsing
- [miette](https://github.com/zkat/miette) — Diagnostic rendering
- [clap](https://github.com/clap-rs/clap) — CLI framework
