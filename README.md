# sqlsurge

[![CI](https://github.com/yukikotani231/sqlsurge/actions/workflows/ci.yml/badge.svg)](https://github.com/yukikotani231/sqlsurge/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/sqlsurge-cli.svg)](https://crates.io/crates/sqlsurge-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**SQL static analyzer that validates queries against schema definitions — no database connection required.**

sqlsurge parses your schema DDL files and validates SQL queries at build time, catching errors like missing tables, unknown columns, and typos before they reach production.

> **Note:** sqlsurge is in early development (alpha). APIs and diagnostics may change between versions. Feedback and contributions are welcome!

## Features

- **Zero database dependency** — Works entirely offline using schema SQL files
- **Framework agnostic** — Works with Rails, Prisma, raw SQL migrations, and more
- **Helpful diagnostics** — Clear error messages with suggestions for typos
- **CI-ready** — JSON and SARIF output formats for integration with CI/CD pipelines
- **Fast** — Built in Rust for speed

## Installation

### via npm (Recommended)

```bash
npm install -g sqlsurge-cli
```

Or use directly with `npx`:

```bash
npx sqlsurge-cli check --schema schema.sql query.sql
```

### via Cargo

```bash
cargo install sqlsurge-cli
```

### From GitHub Releases

Download the latest binary from [Releases](https://github.com/yukikotani231/sqlsurge/releases).

## Quick Start

```bash
# Validate queries against a schema file
sqlsurge check --schema schema.sql queries/*.sql

# Use multiple schema files
sqlsurge check -s users.sql -s orders.sql queries/*.sql

# Use a migrations directory
sqlsurge check --schema-dir ./migrations queries/*.sql
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

sqlsurge will report:

```
error[E0002]: Column 'naem' not found
   = help: Did you mean 'name'?

error[E0002]: Column 'user_id' not found
```

## Framework Integration

### Prisma

Prisma generates SQL migration files automatically:

```bash
sqlsurge check --schema-dir prisma/migrations queries/*.sql
```

### Rails

With `config.active_record.schema_format = :sql`:

```bash
sqlsurge check --schema db/structure.sql queries/*.sql
```

Or with SQL migrations:

```bash
sqlsurge check --schema-dir db/migrate queries/*.sql
```

### Raw SQL

Just point to your schema files:

```bash
sqlsurge check --schema schema/*.sql queries/**/*.sql
```

## Diagnostic Rules

| Code | Name | Description |
|------|------|-------------|
| E0001 | table-not-found | Referenced table does not exist in schema |
| E0002 | column-not-found | Referenced column does not exist in table |
| E0003 | type-mismatch | Type incompatibility in expressions |
| E0004 | potential-null-violation | Possible NOT NULL constraint violation |
| E0005 | column-count-mismatch | INSERT column count doesn't match values |
| E0006 | ambiguous-column | Column reference is ambiguous across tables |
| E0007 | join-type-mismatch | JOIN condition compares incompatible types |

## CLI Reference

```
sqlsurge check [OPTIONS] <FILES>...

Arguments:
  <FILES>...                SQL files to validate (supports glob patterns)

Options:
  -s, --schema <FILE>       Schema definition file (can be specified multiple times)
      --schema-dir <DIR>    Directory containing schema files
  -c, --config <FILE>       Path to configuration file [default: sqlsurge.toml]
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
sqlsurge check -s schema.sql -f json queries/*.sql
```

### SARIF (for GitHub Code Scanning)

```bash
sqlsurge check -s schema.sql -f sarif queries/*.sql > results.sarif
```

## Supported SQL Dialects

- **PostgreSQL** (default)
- MySQL (planned)
- SQLite (planned)

## Roadmap

- [x] Configuration file (`sqlsurge.toml`)
- [ ] LSP server for editor integration
- [ ] MySQL and SQLite dialect support
- [ ] Type inference for expressions
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
