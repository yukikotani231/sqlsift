# sqlsift

[![CI](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsift/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/sqlsift-cli.svg)](https://crates.io/crates/sqlsift-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**SQL static analyzer that validates queries against schema definitions — no database connection required.**

sqlsift parses your DDL files (CREATE TABLE, CREATE VIEW, CREATE TYPE, ALTER TABLE, etc.) and validates SQL queries at build time, catching errors like missing tables, unknown columns, and typos before they reach production.

> **Note:** sqlsift is in early development (alpha). APIs and diagnostics may change between versions. Feedback and contributions are welcome!

## Features

- **Zero database dependency** — Works entirely offline using schema SQL files
- **Framework agnostic** — Works with Rails, Prisma, raw SQL migrations, and more
- **Helpful diagnostics** — Clear error messages with suggestions for typos
- **CI-ready** — JSON and SARIF output formats for integration with CI/CD pipelines
- **Fast** — Built in Rust for speed

## Installation

### via npm (Recommended)

```bash
npm install -g sqlsift-cli
```

Or use directly with `npx`:

```bash
npx sqlsift-cli check --schema schema.sql query.sql
```

### via Cargo

```bash
cargo install sqlsift-cli
```

### From GitHub Releases

Download the latest binary from [Releases](https://github.com/yukikotani231/sqlsift/releases).

## Quick Start

```bash
# Validate queries against a schema file
sqlsift check --schema schema.sql queries/*.sql

# Use multiple schema files
sqlsift check -s users.sql -s orders.sql queries/*.sql

# Use a migrations directory
sqlsift check --schema-dir ./migrations queries/*.sql
```

## Example

Given a schema:

```sql
-- schema.sql
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    email TEXT UNIQUE
);
```

And a query with errors:

```sql
-- query.sql
SELECT naem, user_id FROM users;
```

sqlsift will report:

```
error[E0002]: Column 'naem' not found
   = help: Did you mean 'name'?

error[E0002]: Column 'user_id' not found
```

## Framework Integration

### Prisma

Prisma generates SQL migration files automatically:

```bash
sqlsift check --schema-dir prisma/migrations queries/*.sql
```

### Rails

With `config.active_record.schema_format = :sql`:

```bash
sqlsift check --schema db/structure.sql queries/*.sql
```

Or with SQL migrations:

```bash
sqlsift check --schema-dir db/migrate queries/*.sql
```

### Raw SQL

Just point to your schema files:

```bash
sqlsift check --schema schema/*.sql queries/**/*.sql
```

## Diagnostic Rules

| Code | Name | Description | Status |
|------|------|-------------|--------|
| E0001 | table-not-found | Referenced table does not exist in schema | ✅ Implemented |
| E0002 | column-not-found | Referenced column does not exist in table | ✅ Implemented |
| E0003 | type-mismatch | Type incompatibility in expressions (comparisons, arithmetic) | ✅ Implemented |
| E0004 | potential-null-violation | Possible NOT NULL constraint violation | 🚧 Reserved |
| E0005 | column-count-mismatch | INSERT column count doesn't match values | ✅ Implemented |
| E0006 | ambiguous-column | Column reference is ambiguous across tables | ✅ Implemented |
| E0007 | join-type-mismatch | JOIN condition compares incompatible types | ✅ Implemented |

### Type Inference Coverage (E0003, E0007)

**Currently Detected:**
- ✅ WHERE clause comparisons (`WHERE id = 'text'`)
- ✅ Arithmetic operations (`SELECT name + 10`)
- ✅ JOIN conditions (`ON users.id = orders.user_name`)
- ✅ INSERT value type mismatches (`INSERT INTO users (id) VALUES ('text')`)
- ✅ UPDATE assignment type mismatches (`UPDATE users SET id = 'text'`)
- ✅ Nested expressions (`WHERE (a + b) * 2 = 'text'`)
- ✅ All comparison operators (=, !=, <, >, <=, >=)
- ✅ Numeric type compatibility (INTEGER, BIGINT, DECIMAL, etc.)

**Not Yet Detected:**
- ⏳ CAST expression type inference
- ⏳ Function return types (COUNT, SUM, AVG, etc.)
- ⏳ CASE expression type consistency
- ⏳ Subquery/CTE column type inference

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

## CLI Reference

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
      --max-errors <N>      Maximum number of errors before stopping [default: 100]
  -v, --verbose             Enable verbose output
  -q, --quiet               Suppress non-error output
  -h, --help                Print help
```

## Output Formats

### Human (default)

```
error[E0002]: Column 'user_id' not found in table 'users'
  --> queries/fetch.sql:3:12
   |
 3 |   WHERE users.user_id = $1
   |              ^^^^^^^^^
   |
   = help: Did you mean 'id'?
```

### JSON

```bash
sqlsift check -s schema.sql -f json queries/*.sql
```

### SARIF (for GitHub Code Scanning)

```bash
sqlsift check -s schema.sql -f sarif queries/*.sql > results.sarif
```

## CI Integration

### GitHub Actions

Add sqlsift to your CI pipeline to catch SQL errors in pull requests:

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

Upload results to GitHub's Security tab:

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

## Supported SQL Queries

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

## Supported DDL

- `CREATE TABLE` (columns, constraints, primary keys, foreign keys, UNIQUE)
- `CREATE VIEW` (column inference from SELECT projection)
- `CREATE TYPE AS ENUM`
- `ALTER TABLE` (ADD/DROP/RENAME COLUMN, ADD CONSTRAINT, RENAME TABLE)
- `CHECK` constraints (column-level and table-level)
- `GENERATED AS IDENTITY` columns (ALWAYS / BY DEFAULT)
- Resilient parsing — unsupported DDL (functions, triggers, domains, etc.) is gracefully skipped

## Supported SQL Dialects

- **PostgreSQL** (default) — fully supported
- **MySQL** — supported (`--dialect mysql`)
- **SQLite** — supported (`--dialect sqlite`)

Use the `--dialect` flag to specify the dialect.

## Roadmap

### Completed
- [x] Configuration file (`sqlsift.toml`)
- [x] MySQL dialect support
- [x] SQLite dialect support
- [x] Type inference for expressions (WHERE, JOIN, arithmetic, INSERT/UPDATE)
- [x] LSP server for editor integration (VS Code extension)

### Planned
- [ ] CAST expression type inference
- [ ] Function return type inference (COUNT, SUM, AVG, etc.)
- [ ] CASE expression type consistency checking
- [ ] Subquery/CTE column type inference
- [ ] Custom rule plugins

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Run tests (`cargo test`)
4. Commit your changes (`git commit -m 'Add amazing feature'`)
5. Push to the branch (`git push origin feature/amazing-feature`)
6. Open a Pull Request

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs) — SQL parsing
- [miette](https://github.com/zkat/miette) — Diagnostic rendering
- [clap](https://github.com/clap-rs/clap) — CLI framework
