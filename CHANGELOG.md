# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.3] - 2026-02-08

### Added
- **`--dialect` flag wired up**: CLI `--dialect` option now correctly configures the SQL parser dialect (previously ignored)
- **Real-world schema test fixtures**: Chinook, Pagila, Northwind schemas with comprehensive valid/invalid query tests covering SELECT, JOIN, INSERT, UPDATE, DELETE, subqueries, and CTEs
- Third-party license file for test fixtures

### Fixed
- `--dialect` CLI flag was completely ignored; PostgreSQL dialect was hardcoded throughout
- ALTER TABLE warnings for non-schema-affecting operations (e.g., `OWNER TO`) are now suppressed

## [0.1.0-alpha.2] - 2026-02-08

### Added
- **CHECK constraints**: Column-level and table-level CHECK constraint parsing and storage
- **CREATE TYPE AS ENUM**: Enum type definitions with value storage in catalog
- **GENERATED AS IDENTITY**: ALWAYS and BY DEFAULT identity columns with implicit NOT NULL
- **CREATE VIEW**: View definitions with column inference from SELECT projection, wildcard expansion, and query-time resolution
- **ALTER TABLE**: ADD COLUMN, DROP COLUMN, RENAME COLUMN, RENAME TABLE, ADD CONSTRAINT support
- **Resilient SQL parsing**: Gracefully skip unsupported DDL statements (CREATE FUNCTION, CREATE TRIGGER, CREATE DOMAIN, etc.) instead of failing the entire schema file
- Real-world test fixtures from Sakila and webknossos schemas

### Fixed
- Schema files with mixed supported/unsupported SQL statements now parse correctly
- Comments preceding DDL statements no longer cause statement-by-statement parsing to skip valid statements

### Changed
- Known Limitations updated: VIEWs are now supported; ALTER TABLE is now supported

## [0.1.0-alpha.1] - 2026-02-07

### Added
- Initial release of sqlsurge
- SQL static analysis against schema definitions
- Support for PostgreSQL dialect
- Schema parsing from CREATE TABLE statements
- Query validation for SELECT, INSERT, UPDATE, DELETE statements
- Error detection:
  - E0001: Table not found
  - E0002: Column not found
  - E0003: Type mismatch (reserved)
  - E0004: Potential NULL violation (reserved)
  - E0005: Column count mismatch in INSERT
  - E0006: Ambiguous column reference
  - E0007: JOIN type mismatch (reserved)
  - E1000: Parse error
- JOIN condition validation
- Subquery support (including correlated subqueries)
- CTE (Common Table Expressions) support
- Error position reporting (line and column numbers)
- Multiple output formats: human-readable, JSON, SARIF
- Configuration file support (sqlsurge.toml)
- Rule disabling via CLI (--disable) or config file
- CLI with check, schema, and parse commands
- Typo suggestions using Levenshtein distance
- CI/CD integration support via exit codes and SARIF output
- Framework integration examples (Rails, Prisma)

### Known Limitations
- Only PostgreSQL dialect fully supported
- Type checking is basic (existence only, not full type inference)
- No support for VIEWs, functions, or stored procedures
- Derived table (subquery in FROM) column resolution is incomplete

[Unreleased]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.3...HEAD
[0.1.0-alpha.3]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.2...v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/yukikotani231/sqlsurge/releases/tag/v0.1.0-alpha.1
