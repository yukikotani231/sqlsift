# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/claude-code) when working with this repository.

## Project Overview

sqlsift is a SQL static analyzer that validates queries against schema definitions without requiring a database connection. It parses DDL statements (CREATE TABLE, CREATE VIEW, CREATE TYPE, ALTER TABLE) to build an in-memory schema catalog, then validates SQL queries (SELECT, INSERT, UPDATE, DELETE) against that catalog.

## Architecture

```
sqlsift/
├── crates/
│   ├── sqlsift-core/     # Core library (schema parsing, analysis engine)
│   │   ├── schema/        # Schema catalog and DDL parsing
│   │   ├── analyzer/      # Query validation and name resolution
│   │   ├── types/         # SQL type system
│   │   ├── dialect/       # SQL dialect abstraction
│   │   └── error.rs       # Diagnostic types
│   │
│   ├── sqlsift-cli/      # CLI binary
│   │   ├── args.rs        # CLI argument definitions (clap)
│   │   ├── config.rs      # Configuration file (sqlsift.toml) support
│   │   ├── output/        # Output formatters (human, JSON, SARIF)
│   │   └── main.rs        # Entry point
│   │
│   └── sqlsift-lsp/      # LSP server binary
│       ├── server.rs      # LanguageServer trait implementation (tower-lsp)
│       ├── state.rs       # Server state (catalog, config, open documents)
│       ├── config.rs      # sqlsift.toml loader
│       ├── diagnostics.rs # sqlsift Diagnostic → LSP Diagnostic conversion
│       └── main.rs        # Entry point (stdin/stdout transport)
│
├── editors/
│   └── vscode/            # VS Code extension (LSP client)
│       ├── src/extension.ts
│       └── package.json
│
├── tests/fixtures/        # Test SQL files
│   └── real-world/        # Real-world schema test fixtures (Chinook, Pagila, Northwind)
├── scripts/
│   └── release.sh         # Release automation script
├── dist-workspace.toml    # cargo-dist configuration for releases
├── sqlsift.toml          # Sample configuration file
├── CHANGELOG.md           # Version history
└── PUBLISHING.md          # Release guide
```

### Key Components

1. **SchemaBuilder** (`schema/builder.rs`): Parses DDL statements (CREATE TABLE, CREATE VIEW, CREATE TYPE, ALTER TABLE) using sqlparser-rs and builds a `Catalog`. Supports resilient parsing to skip unsupported syntax.
2. **Catalog** (`schema/catalog.rs`): In-memory representation of database schema (tables, columns, constraints, views, enums)
3. **Analyzer** (`analyzer/mod.rs`): Entry point for query validation (61 comprehensive tests)
4. **NameResolver** (`analyzer/resolver.rs`): Resolves table, view, and column references, supports CTEs with scope isolation
5. **SqlType** (`types/mod.rs`): Internal SQL type representation with compatibility checking
6. **Config** (`config.rs`): Configuration file loader with hierarchical merging (file < CLI args)
7. **LSP Backend** (`sqlsift-lsp/server.rs`): tower-lsp LanguageServer implementation with real-time diagnostics
8. **ServerState** (`sqlsift-lsp/state.rs`): LSP server state management (catalog, config, open documents)

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

# Run tests (61 tests covering DDL parsing, SELECT, INSERT, UPDATE, DELETE, CTEs, subqueries, VIEWs)
cargo test

# Run with example
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/valid_query.sql

# Check for errors
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/invalid_query.sql

# Use configuration file
cargo run -- check queries/*.sql  # Auto-discovers sqlsift.toml

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
3. Document in `sqlsift.toml` sample file

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
- Real-world schema tests in `tests/fixtures/real-world/` (Chinook, Pagila, Northwind) with valid and invalid query files
- Test both positive cases (valid SQL) and negative cases (should produce diagnostics)
- Comprehensive test coverage: 71 unit tests + 72 PostgreSQL pattern tests + 80 MySQL real-world queries covering DDL parsing, SELECT, INSERT, UPDATE, DELETE, CTEs, subqueries, VIEWs, ALTER TABLE, derived tables, window functions, and advanced expressions
- Test-driven development (TDD) approach: write failing tests first, then implement features

## Style Guidelines

- Follow Rust standard formatting (`cargo fmt`)
- Use `cargo clippy` for linting
- Prefer explicit error handling over `.unwrap()` in library code
- Document public APIs with doc comments
- Error messages should be actionable (include suggestions when possible)

## Current Limitations

### SQL Dialect Support
- Schema-qualified names (e.g., `public.users`) are not fully resolved

### Type Inference (Partial Implementation)
**Implemented (E0003, E0007):**
- WHERE clause type checking (comparisons, arithmetic)
- JOIN condition type checking
- Binary operator type validation (=, <, >, <=, >=, !=, +, -, *, /, %)
- Nested expression type inference
- Numeric type compatibility (TINYINT → BIGINT implicit casts)
- INSERT VALUES type checking (`INSERT INTO users (id) VALUES ('text')` → E0003)
- UPDATE SET type checking (`UPDATE users SET id = 'text'` → E0003)

**Not Yet Implemented (TODO):**
- CAST expression type inference (`CAST(x AS INTEGER)`)
- Function return type inference (COUNT → INTEGER, SUM → NUMERIC, etc.)
- CASE expression type consistency (THEN/ELSE must have compatible types)
- Subquery/CTE column type inference
- VIEW column type inference from SELECT projection

**Implementation Notes:**
- Current type inference covers ~85% of real-world type errors
- See `crates/sqlsift-core/src/analyzer/type_resolver.rs` for implementation

### Other Limitations
- Functions and stored procedures are skipped (not analyzed)
- UNION/INTERSECT/EXCEPT column count validation not implemented

## Supported Features

- ✅ SELECT, INSERT, UPDATE, DELETE statements
- ✅ CTEs (WITH clause) with proper scope isolation, including recursive CTEs
- ✅ JOINs (INNER, LEFT, RIGHT, FULL, CROSS, NATURAL)
- ✅ Subqueries (WHERE IN/EXISTS, FROM derived tables, scalar subqueries)
- ✅ LATERAL vs non-LATERAL scope isolation
- ✅ Column and table name resolution with ORDER BY alias support
- ✅ UPDATE ... FROM / DELETE ... USING (PostgreSQL extensions)
- ✅ Window functions (OVER, PARTITION BY, ORDER BY, ROWS/RANGE frames)
- ✅ Aggregate FILTER clause
- ✅ GROUPING SETS, CUBE, ROLLUP
- ✅ DISTINCT ON (PostgreSQL-specific)
- ✅ UNION / INTERSECT / EXCEPT with column inference
- ✅ Table-valued functions in FROM (generate_series, etc.)
- ✅ Comprehensive expression resolution (CASE, CAST, EXTRACT, JSON operators, AT TIME ZONE, ARRAY, etc.)
- ✅ CREATE VIEW with column inference and wildcard expansion
- ✅ ALTER TABLE (ADD/DROP/RENAME COLUMN, ADD CONSTRAINT, RENAME TABLE)
- ✅ CREATE TYPE AS ENUM
- ✅ CHECK constraints (column-level and table-level)
- ✅ GENERATED AS IDENTITY columns
- ✅ Resilient parsing (gracefully skips unsupported DDL)
- ✅ Configuration file (sqlsift.toml)
- ✅ Rule disabling (--disable flag)
- ✅ Multiple output formats (human, JSON, SARIF)
- ✅ Type inference for expressions (WHERE, JOIN, binary operators, nested expressions)
  - Detects type mismatches in comparisons (E0003)
  - Detects JOIN condition type incompatibilities (E0007)
  - Supports numeric type compatibility (implicit casts)
  - See "Current Limitations" for partial implementation scope

## Error Codes

- **E0001**: Table not found
- **E0002**: Column not found
- **E0003**: Type mismatch (comparisons, arithmetic operations)
- **E0004**: Potential NULL violation (reserved, not yet implemented)
- **E0005**: Column count mismatch in INSERT
- **E0006**: Ambiguous column reference
- **E0007**: JOIN type mismatch (JOIN condition type incompatibility)
- **E1000**: Generic parse error

## Release Process

```bash
# 1. Run the release script (bumps version, updates CHANGELOG, creates PR)
./scripts/release.sh <version>

# 2. Merge the PR on GitHub
#    -> auto-tag.yml creates git tag automatically
#    -> release.yml (cargo-dist) builds and publishes
```

- npm package: `sqlsift-cli` (provides `sqlsift` command)
- Supported platforms: macOS (x64/ARM64), Linux (x64/ARM64), Windows (x64)
- See `PUBLISHING.md` for details
