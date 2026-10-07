# sqlsift

[![CI](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/sqlsift-cli.svg)](https://www.npmjs.com/package/sqlsift-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Catch broken SQL before it reaches production — without a database.**

sqlsift reads your schema (`CREATE TABLE`, migrations, `structure.sql`, …) and checks your raw SQL queries against it: missing tables, typo'd columns, type mismatches, wrong `INSERT` arity, ambiguous columns. It runs offline in milliseconds, so it fits in pre-commit hooks, CI, and your editor.

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

Prebuilt binaries are also available on the [Releases](https://github.com/yukikotani231/sqlsift/releases) page.

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
# disable = ["E0006"]
```

Then just run `sqlsift check queries/**/*.sql`.

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

### GitHub Actions

```yaml
# .github/workflows/sqlsift.yml
name: SQL Lint
on: [push, pull_request]
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: npx sqlsift-cli check --schema schema.sql queries/*.sql
```

### GitHub Code Scanning (SARIF)

Show errors inline on pull requests and in the Security tab:

```yaml
# .github/workflows/sqlsift.yml
name: SQL Lint
on: [push, pull_request]
jobs:
  sqlsift:
    runs-on: ubuntu-latest
    permissions:
      security-events: write
    steps:
      - uses: actions/checkout@v4
      - run: npx sqlsift-cli check -s schema.sql -f sarif queries/*.sql > results.sarif
        continue-on-error: true
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: results.sarif
```

JSON output is also available with `--format json`.

## Diagnostic Rules

| Code | Name | Description | Status |
|------|------|-------------|--------|
| E0001 | table-not-found | Referenced table does not exist in schema | ✅ Implemented |
| E0002 | column-not-found | Referenced column does not exist in table | ✅ Implemented |
| E0003 | type-mismatch | Type incompatibility in expressions (comparisons, arithmetic) | ✅ Implemented |
| E0004 | potential-null-violation | Potential NOT NULL violation (explicit NULL assignment) | ✅ Implemented |
| E0005 | column-count-mismatch | INSERT column count doesn't match values | ✅ Implemented |
| E0006 | ambiguous-column | Column reference is ambiguous across tables | ✅ Implemented |
| E0007 | join-type-mismatch | JOIN condition compares incompatible types | ✅ Implemented |

### Inline Suppression

Suppress diagnostics on specific lines using SQL comments:

```sql
-- Suppress a specific rule on the next line
-- sqlsift:disable E0002
SELECT legacy_col FROM users;

-- Suppress on the same line
SELECT legacy_col FROM users; -- sqlsift:disable E0002

-- Suppress multiple rules
SELECT bad_col FROM missing_table; -- sqlsift:disable E0001, E0002

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

**Not Yet Detected:**
- ⏳ CASE expression type consistency
- ⏳ Subquery/CTE column type inference

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
      --disable <RULE>      Disable specific rules (e.g., E0001, E0002)
  -d, --dialect <NAME>      SQL dialect [default: postgresql]
  -f, --format <FORMAT>     Output format: human, json, sarif [default: human]
      --max-errors <N>      Maximum number of errors before stopping [default: 100, 0 = unlimited]
  -v, --verbose             Enable verbose logging (-vv for debug)
  -q, --quiet               Suppress summary/non-error output
  -h, --help                Print help
```

</details>

## Roadmap

#### Completed
- [x] Configuration file (`sqlsift.toml`)
- [x] MySQL dialect support
- [x] SQLite dialect support
- [x] Type inference for expressions (WHERE, JOIN, arithmetic, INSERT/UPDATE)
- [x] LSP server for editor integration (VS Code extension)

#### Planned
- [ ] CASE expression type consistency checking
- [ ] Subquery/CTE column type inference
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
