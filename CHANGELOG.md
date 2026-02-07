# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/yukikotani231/sqlsurge/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/yukikotani231/sqlsurge/releases/tag/v0.1.0-alpha.1
