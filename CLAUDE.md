# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/claude-code) when working with this repository.

## Project Overview

sqlsurge is a SQL static analyzer that validates queries against schema definitions without requiring a database connection. It parses CREATE TABLE statements to build an in-memory schema catalog, then validates SQL queries (SELECT, INSERT, UPDATE, DELETE) against that catalog.

## Architecture

```
sqlsurge/
├── crates/
│   ├── sqlsurge-core/     # Core library (schema parsing, analysis engine)
│   │   ├── schema/        # Schema catalog and DDL parsing
│   │   ├── analyzer/      # Query validation and name resolution
│   │   ├── types/         # SQL type system
│   │   ├── dialect/       # SQL dialect abstraction
│   │   └── error.rs       # Diagnostic types
│   │
│   └── sqlsurge-cli/      # CLI binary
│       ├── args.rs        # CLI argument definitions (clap)
│       ├── config.rs      # Configuration file (sqlsurge.toml) support
│       ├── output/        # Output formatters (human, JSON, SARIF)
│       └── main.rs        # Entry point
│
├── tests/fixtures/        # Test SQL files
├── dist-workspace.toml    # cargo-dist configuration for releases
├── sqlsurge.toml          # Sample configuration file
├── CHANGELOG.md           # Version history
└── PUBLISHING.md          # Release guide
```

### Key Components

1. **SchemaBuilder** (`schema/builder.rs`): Parses DDL statements (CREATE TABLE, CREATE VIEW, CREATE TYPE, ALTER TABLE) using sqlparser-rs and builds a `Catalog`. Supports resilient parsing to skip unsupported syntax.
2. **Catalog** (`schema/catalog.rs`): In-memory representation of database schema (tables, columns, constraints, views, enums)
3. **Analyzer** (`analyzer/mod.rs`): Entry point for query validation (57 comprehensive tests)
4. **NameResolver** (`analyzer/resolver.rs`): Resolves table, view, and column references, supports CTEs with scope isolation
5. **SqlType** (`types/mod.rs`): Internal SQL type representation with compatibility checking
6. **Config** (`config.rs`): Configuration file loader with hierarchical merging (file < CLI args)

### Data Flow

```
Schema SQL → sqlparser → AST → SchemaBuilder → Catalog
                                                  ↓
Query SQL  → sqlparser → AST → Analyzer → NameResolver → Diagnostics
```

## Build & Test Commands

```bash
# Build
cargo build

# Run tests (57 tests covering DDL parsing, SELECT, INSERT, UPDATE, DELETE, CTEs, subqueries, VIEWs)
cargo test

# Run with example
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/valid_query.sql

# Check for errors
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/invalid_query.sql

# Use configuration file
cargo run -- check queries/*.sql  # Auto-discovers sqlsurge.toml

# Disable specific error codes
cargo run -- check --disable E0002 --schema schema.sql query.sql

# Output formats
cargo run -- check --format json --schema schema.sql query.sql
cargo run -- check --format sarif --schema schema.sql query.sql
```

## Code Patterns

### Adding a New Diagnostic Rule

1. Add variant to `DiagnosticKind` in `error.rs`
2. Implement detection logic in `analyzer/resolver.rs` or create a new rule module
3. Add test case in `analyzer/mod.rs`

### Adding SQL Type Support

1. Add variant to `SqlType` enum in `types/mod.rs`
2. Update `SqlType::from_ast()` to handle the new sqlparser DataType
3. Update `SqlType::display_name()` for human-readable output
4. Update `is_compatible_with()` if needed for type coercion

### Adding CLI Options

1. Add field to appropriate struct in `args.rs` using clap derive macros
2. Add corresponding field to `Config` struct in `config.rs` if it should be configurable via file
3. Update `Config::merge_with_args()` to handle CLI override
4. Handle the option in `main.rs`

### Adding Configuration File Options

1. Add field to `Config` struct in `config.rs` with `#[serde(default)]`
2. Update `Config::merge_with_args()` to merge with CLI arguments
3. Document in `sqlsurge.toml` sample file

## Dependencies

- **sqlparser** (0.53): SQL parsing (PostgreSQL dialect)
- **clap** (4.5): CLI argument parsing with derive macros
- **miette** (7.4): Diagnostic rendering with fancy formatting
- **thiserror** (2.0): Error type derivation
- **serde** (1.0): Serialization for JSON/TOML
- **toml** (0.8): Configuration file parsing
- **glob** (0.3): File pattern matching
- **indexmap** (2.7): Ordered maps for deterministic output

## Testing Strategy

- Unit tests are colocated with modules (`#[cfg(test)] mod tests`)
- Integration tests use SQL fixtures in `tests/fixtures/`
- Test both positive cases (valid SQL) and negative cases (should produce diagnostics)
- Comprehensive test coverage: 57 tests covering DDL parsing, SELECT, INSERT, UPDATE, DELETE, CTEs, subqueries, VIEWs, ALTER TABLE
- Test-driven development (TDD) approach: write failing tests first, then implement features

## Style Guidelines

- Follow Rust standard formatting (`cargo fmt`)
- Use `cargo clippy` for linting
- Prefer explicit error handling over `.unwrap()` in library code
- Document public APIs with doc comments
- Error messages should be actionable (include suggestions when possible)

## Current Limitations

- Only PostgreSQL dialect is fully supported
- Subquery column resolution is incomplete (basic support exists)
- Type checking is basic (existence only, not full type inference)
- No support for window functions, GROUPING SETS, or advanced SQL features
- Functions and stored procedures are skipped (not analyzed)

## Supported Features

- ✅ SELECT, INSERT, UPDATE, DELETE statements
- ✅ CTEs (WITH clause) with proper scope isolation
- ✅ JOINs (INNER, LEFT, RIGHT, FULL, CROSS)
- ✅ Subqueries (WHERE, FROM)
- ✅ Column and table name resolution
- ✅ CREATE VIEW with column inference and wildcard expansion
- ✅ ALTER TABLE (ADD/DROP/RENAME COLUMN, ADD CONSTRAINT, RENAME TABLE)
- ✅ CREATE TYPE AS ENUM
- ✅ CHECK constraints (column-level and table-level)
- ✅ GENERATED AS IDENTITY columns
- ✅ Resilient parsing (gracefully skips unsupported DDL)
- ✅ Configuration file (sqlsurge.toml)
- ✅ Rule disabling (--disable flag)
- ✅ Multiple output formats (human, JSON, SARIF)

## Error Codes

- **E0001**: Table not found
- **E0002**: Column not found
- **E0003**: Parse error
- **E0004**: Ambiguous table reference
- **E0005**: Column count mismatch in INSERT
- **E0006**: Ambiguous column reference
- **E1000**: Generic parse error

## Distribution

The project uses **cargo-dist** for automated releases:
- npm package: `sqlsurge-cli` (provides `sqlsurge` command)
- Supported platforms: macOS (x64/ARM64), Linux (x64/ARM64), Windows (x64)
- GitHub Actions workflow auto-publishes on version tag push
- See `PUBLISHING.md` for release guide
