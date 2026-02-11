# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.7] - 2026-02-11

### Added
- **LSP server** (`sqlsurge-lsp`): Language Server Protocol support for real-time SQL diagnostics in editors
  - textDocument/didOpen, didChange, didSave, didClose
  - Automatic schema loading from `sqlsurge.toml`
  - Schema file change detection with catalog rebuild
  - Diagnostic severity, error codes, and help messages
  - 12 unit tests (diagnostic conversion, state management)
- **VS Code extension** (`editors/vscode/`): Thin LSP client for VS Code
  - Configurable server path via `sqlsurge.serverPath` setting
  - SQL language configuration (comments, brackets)
- **Neovim support**: Works out of the box with built-in LSP client

### Fixed
- `SELECT *` diagnostic now points to the `SELECT` keyword instead of file start

## [0.1.0-alpha.6] - 2026-02-08

### Added
- **Type inference engine**: SQL expression type checking for WHERE clauses and JOIN conditions
  - E0003 (type-mismatch): Detect incompatible type comparisons (e.g., `WHERE id = 'text'`)
  - E0007 (join-type-mismatch): Detect JOIN condition type incompatibilities (e.g., `ON users.id = orders.name`)
  - Binary operator type validation: comparisons (=, !=, <, >, <=, >=) and arithmetic (+, -, *, /)
  - Nested expression type inference: `(a + b) * 2 = c`
  - Numeric type compatibility: implicit casts between TINYINT, SMALLINT, INTEGER, BIGINT

### Changed
- Reorganized test suite: moved integration tests to `tests/analyzer_tests.rs` (74 tests)
- Improved API documentation with doc-test examples
- Replaced `unwrap()` with `expect()` in catalog code for better error messages

## [0.1.0-alpha.5] - 2026-02-08

### Added
- **MySQL dialect support**: Full schema parsing and query validation for MySQL
- MySQL-specific types: `TINYINT`, `MEDIUMINT`, `UNSIGNED` integer variants, `DATETIME`, inline `ENUM`
- `AUTO_INCREMENT` handling with implicit NOT NULL inference
- 10 MySQL unit tests covering schema parsing, SELECT, JOIN, INSERT, UPDATE, DELETE, subquery, CTE, and error detection
- Real-world MySQL test fixtures:
  - **Sakila** (BSD): 16 tables, 40 valid queries, 12 error detection tests
  - **Chinook MySQL** (MIT): 11 tables, 40 valid queries, 12 error detection tests

## [0.1.0-alpha.4] - 2026-02-08

### Added
- **Derived table (subquery in FROM) support**: Resolve aliases and validate column references for `FROM (SELECT ...) AS sub`
- **LATERAL vs non-LATERAL scope isolation**: Non-LATERAL subqueries correctly cannot see outer FROM tables
- **UPDATE ... FROM / DELETE ... USING**: PostgreSQL-specific multi-table update/delete syntax
- **Recursive CTE support**: CTEs can reference themselves in recursive queries
- **Table-valued functions in FROM**: `generate_series()`, `unnest()` etc. recognized as table sources
- **UNION/INTERSECT/EXCEPT column inference**: Infer output columns from set operations for CTE/derived table validation
- **Comprehensive expression resolution**: AtTimeZone, Collate, Ceil/Floor, Overlay, IsDistinctFrom, IsUnknown, SimilarTo, Tuple, Array, Subscript, Method, GroupingSets/Cube/Rollup
- **Function FILTER/OVER clause resolution**: Validate column references in `COUNT(*) FILTER (WHERE ...)` and `OVER (PARTITION BY ... ORDER BY ...)`
- **ORDER BY column resolution**: Validate ORDER BY references including SELECT alias support
- **Named function argument resolution**: Handle `func(name => value)` syntax
- 72 PostgreSQL pattern test fixtures (basic, advanced, and expression coverage)

### Fixed
- WHERE subquery scope leak: subqueries in IN/EXISTS no longer pollute outer table scope
- VALUES derived table column aliases now correctly applied
- Empty derived_columns (table-valued functions) no longer cause false column-not-found errors

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

[Unreleased]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.7...HEAD
[0.1.0-alpha.7]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.6...v0.1.0-alpha.7
[0.1.0-alpha.6]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.5...v0.1.0-alpha.6
[0.1.0-alpha.5]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.4...v0.1.0-alpha.5
[0.1.0-alpha.4]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.3...v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.2...v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/yukikotani231/sqlsurge/releases/tag/v0.1.0-alpha.1
