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
│   │   ├── rules.rs       # Rule registry, categories and rule levels
│   │   ├── baseline.rs    # Baseline file (known diagnostics), shared by CLI and LSP
│   │   ├── impact.rs      # `sqlsift impact`: catalog diff (changed relations) and diagnostic diff
│   │   ├── unused.rs      # `sqlsift unused`: tables and columns no query reads (from analyzer references)
│   │   └── error.rs       # Diagnostic types
│   │
│   ├── sqlsift-cli/      # CLI binary
│   │   ├── args.rs        # CLI argument definitions (clap)
│   │   ├── config.rs      # Configuration file (sqlsift.toml) support
│   │   ├── impact.rs      # `sqlsift impact` (schema before/after the migrations, output)
│   │   ├── unused.rs      # `sqlsift unused` (queries and schema views → unused report)
│   │   ├── init/          # `sqlsift init`: project layout detection (detect.rs, sqlc_config.rs), sqlsift.toml rendering
│   │   ├── output/        # Output formatters (human, JSON, SARIF, GitHub Actions)
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
├── docs/                  # User guide (mdBook), published to GitHub Pages at https://sqlsift.dev/docs/
├── site/                  # Browser playground (GitHub Pages root, https://sqlsift.dev/)
├── examples/              # Example projects per stack (sqlc, Prisma, aiosql, psycopg, Go database/sql, postgres.js, sqlx, migrations, MySQL, dbt); CI runs scripts/check-examples.sh on them
├── tests/fixtures/        # Test SQL files
│   └── real-world/        # Pagila schema (used by tests)
├── scripts/
│   └── release.sh         # Release automation script
├── dist-workspace.toml    # cargo-dist configuration for releases
├── sqlsift.toml          # Sample configuration file
├── CHANGELOG.md           # Links to the per-crate changelogs (crates/*/CHANGELOG.md)
└── PUBLISHING.md          # Release guide
```

### Key Components

1. **SchemaBuilder** (`schema/builder.rs`): Parses DDL statements (CREATE TABLE, CREATE VIEW, CREATE TYPE, ALTER TABLE) using sqlparser-rs and builds a `Catalog`. Supports resilient parsing to skip unsupported syntax.
2. **Catalog** (`schema/catalog.rs`): In-memory representation of database schema (tables, columns, constraints, views, enums)
3. **Analyzer** (`analyzer/mod.rs`): Entry point for query validation
4. **Resolver** (`analyzer/resolver.rs`): Walks each statement once: resolves table, view, CTE and column references and reports each query block's output columns with their types; with `Analyzer::with_references` it also records the uses of catalog tables, views and columns (`analyzer::Reference`, reads and writes, for `impact --references` and `sqlsift unused`)
5. **Scope** (`analyzer/scope.rs`): Stack of query blocks (relations, CTEs, USING columns) shared by name resolution and type inference; encodes SQL visibility rules (correlation, non-LATERAL FROM subqueries, CTE scoping)
6. **Type checks** (`analyzer/type_check.rs`): Expression type inference and E0003/E0004/E0007 checks, implemented on `Resolver`
7. **Structural checks** (`analyzer/structure.rs`): E0009-E0015 (duplicate names and target columns, positions, misplaced aggregates, generated columns, ON CONFLICT targets, DISTINCT ORDER BY) and the E0005 checks of column alias lists and subquery shapes; `analyzer/grouping.rs` has E0016 and `analyzer/ranges.rs` E0017, implemented on `Resolver`
8. **SqlType** (`types/mod.rs`): Internal SQL type representation with compatibility checking
9. **Config** (`config.rs`): Configuration file loader with hierarchical merging (file < CLI args)
10. **LSP Backend** (`sqlsift-lsp/server.rs`): tower-lsp LanguageServer implementation with real-time diagnostics
11. **ServerState** (`sqlsift-lsp/state.rs`): LSP server state management (catalog, config, open documents)

### Data Flow

```
Schema SQL → sqlparser → AST → SchemaBuilder → Catalog
                                                  ↓
Query SQL  → sqlparser → AST → Analyzer → Resolver (names + types, one walk) → Diagnostics
```

## Setup

```bash
# Enable pre-commit hooks (fmt + clippy checks)
git config core.hooksPath .githooks
```

## Build & Test Commands

```bash
# Build
cargo build

# Run all workspace tests (unit + integration + LSP)
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

1. Add variant to `DiagnosticKind` in `error.rs` and its entry (code, name, category, summary) to `RULES` in `rules.rs`, in the same position (the registry is indexed by the variant). Give it the next free `E00xx` code: the `E` is a prefix, not a severity, and a code is never changed or reused once released
2. Pick the category by how sure the rule is: `correctness` (definitely wrong, default error), `suspicious` (likely wrong, default warn), `pedantic` / `style` / `restriction` (opt-in)
3. Implement detection logic in `analyzer/resolver.rs` (names), `analyzer/type_check.rs` (types) or `analyzer/structure.rs` (query shape); look names up through `Scope`, never by walking FROM clauses yourself
4. Add test cases (`crates/sqlsift-core/tests/`), the rule to the README rule table and a rule page in `docs/src/rules/` (user guide, mdBook)

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

1. Add field to `Config` struct in `config.rs` with `#[serde(default)]`, and add the key to `KNOWN_KEYS` (unknown keys produce a warning)
2. Update `Config::merge_with_args()` to merge with CLI arguments
3. If the value is a path, resolve it relative to the config file's directory in `Config::from_file()` (and in `sqlsift-lsp/src/state.rs`)
4. Document in `sqlsift.toml` sample file and `docs/src/reference/config.md`

## Dependencies

- **sqlparser** (0.63): SQL parsing (PostgreSQL, MySQL, SQLite; Snowflake, BigQuery, Redshift, Databricks in alpha)
- **clap** (4.5): CLI argument parsing with derive macros
- **miette** (7.4): Diagnostic rendering with fancy formatting
- **thiserror** (2.0): Error type derivation
- **serde** (1.0): Serialization for JSON/TOML
- **toml** (0.8): Configuration file parsing
- **glob** (0.3): File pattern matching
- **indexmap** (2.7): Ordered maps for deterministic output

Dependencies must have a license allowed by `deny.toml` (CI job "Licenses"). After adding, removing or updating a dependency (Cargo or the VS Code extension's npm packages), run `scripts/third-party-licenses.sh` and commit `THIRD-PARTY-LICENSES.md` / `editors/vscode/THIRD-PARTY-NOTICES.md`; they ship in the release archives, the npm package, the VSIX and the playground.

## Testing Strategy

- Unit tests are colocated with modules (`#[cfg(test)] mod tests`)
- Integration tests use SQL fixtures in `tests/fixtures/`
- Real-world SQL: the Pagila schema in `tests/fixtures/real-world/`, and the corpus harness in `tests/corpus/` (public dbt projects and warehouse SQL; not run in CI, its findings become tests in `corpus_regression_tests.rs`)
- Test both positive cases (valid SQL) and negative cases (should produce diagnostics)
- Comprehensive test coverage across unit, integration, doc, and LSP tests covering DDL parsing, SELECT, INSERT, UPDATE, DELETE, CTEs, subqueries, VIEWs, ALTER TABLE, derived tables, window functions, type checking, and the PostgreSQL, MySQL and SQLite dialects (plus the alpha warehouse dialects)
- Test-driven development (TDD) approach: write failing tests first, then implement features

## Style Guidelines

- Follow Rust standard formatting (`cargo fmt`)
- Use `cargo clippy` for linting. Lints are set in `[workspace.lints]` in the root `Cargo.toml` (`clippy::pedantic` minus a few noisy lints); `sqlsift-core` also warns on `unwrap()` / `expect()` outside tests
- Prefer explicit error handling over `.unwrap()` in library code
- Document public APIs with doc comments
- Error messages should be actionable (include suggestions when possible)

## Current Limitations

### Type Inference (Partial Implementation)
**Implemented (E0003, E0007):**
- WHERE clause type checking (comparisons, arithmetic)
- JOIN condition type checking
- Binary operator type validation (=, <, >, <=, >=, !=, +, -, *, /, %)
- Nested expression type inference
- Numeric type compatibility (TINYINT → BIGINT implicit casts)
- INSERT VALUES type checking (`INSERT INTO users (id) VALUES ('text')` → E0003)
- UPDATE SET type checking (`UPDATE users SET id = 'text'` → E0003)
- CAST expression type inference (`CAST(x AS INTEGER)`)
- Function return type inference (e.g., COUNT, SUM, AVG, UPPER, LENGTH, COALESCE)
- String literals are untyped (`ExpressionType::StringLiteral`) and coerce to the other operand's type; only numeric/boolean targets are validated (`SqlType::accepts_string_literal`)
- Date/time arithmetic (`SqlType::temporal_arithmetic_result`)
- Dialect-aware coercions (MySQL/SQLite booleans are integers)
- CASE expression branch consistency and result type (`check_case_branches`, `infer_case_type`)
- Enum literal values for named enums and MySQL inline `ENUM(...)` (`SqlType::Enum`, `report_enum_literal`)
- Column types of CTEs, derived tables, scalar/IN subqueries, RETURNING lists, views and `CREATE TABLE ... AS` (every query block reports typed output columns; views and CTAS use `analyzer::query_output_columns`)
- UNION/INTERSECT/EXCEPT column count and type compatibility (also after `*` expansion)

**Implementation Notes:**
- Anything that can't be inferred is `ExpressionType::Unknown` and never reported
- See `crates/sqlsift-core/src/analyzer/type_check.rs` for implementation

### Other Limitations
- Functions and stored procedures are skipped (not analyzed)

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
- ✅ DDL in query files (CREATE [TEMP] TABLE, CTAS, CREATE VIEW, ALTER TABLE, DROP) is applied by `Analyzer::analyze` to a file-local copy of the catalog via `SchemaBuilder::from_catalog`, visible to later statements of that file only
- ✅ psql scripts (PostgreSQL only, `psql.rs`): meta-commands blanked out, `\g`/`\gset` end a query, `:var`/`:'var'` → `$1`, `:"var"` → identifier whose name diagnostics are dropped; the rewrite keeps every byte offset
- ✅ Schema-qualified names: tables, views and enums are keyed by schema; `table`, `schema.table`, `db.schema.table` (database part ignored) and column references up to `db.schema.table.column` (`split_column_ref`, `Scope::qualified_relation`); backtick-quoted dotted names are split (`split_dotted_idents`). PostgreSQL catalogs are `strict_schemas` (search_path only, E0001 help for a table off the path); MySQL/SQLite catalogs fall back to the default schema / any schema since the current database is chosen at run time; `USE db` sets the database of unqualified names
- ✅ Configuration file (sqlsift.toml), written for an existing project by `sqlsift init` (`sqlsift-cli/src/init/`: detects sqlc, Prisma, Supabase, Drizzle, Flyway, golang-migrate, goose, dbmate, schema dumps, dbt, `.sql` queries and SQL tagged templates, and the dialect; runs a first check as a subprocess with `--format json`)
- ✅ goose `-- +goose Down` and dbmate `-- migrate:down` sections are skipped when loading the schema (`schema/migrations.rs`)
- ✅ dbt / Jinja templates in query files (`templating.rs`: tags are masked keeping locations; `--templating`, `templating = "jinja"`, auto-detected from `dbt_project.yml`)
- ✅ SQL in application code: sqlc `-- name:` query names on diagnostics (`sqlc.rs`), aiosql / HugSQL query names and `:param` placeholders (`named_queries.rs`), SQL in TypeScript/JavaScript tagged templates (`embedded.rs`, `Analyzer::analyze_embedded`, `embedded_sql_tags`), SQL in Rust sqlx macros and `sqlx::` functions (`sqlx.rs`, `Analyzer::analyze_rust`, `embedded_sql_macros`), SQL in Python string literals passed to database calls (`python.rs`, `Analyzer::analyze_python`, `embedded_sql_functions`), SQL in Go string literals passed to `database/sql` / sqlx / pgx calls (`go.rs`, `Analyzer::analyze_go`, `embedded_sql_go_functions`)
- ✅ Rule levels per rule and per category (`[rules]`, `[categories]`, `-A`/`-W`/`-D`, inline `-- sqlsift:disable` and file-wide `-- sqlsift:disable-file`), `sqlsift rules` lists the registry
- ✅ Ignoring query files (`ignore` in sqlsift.toml, `--ignore`; `sqlsift_core::ignore`), honored by the CLI and LSP
- ✅ Baseline of known diagnostics (`sqlsift check --write-baseline`, `--baseline`, `baseline` in sqlsift.toml; `sqlsift_core::baseline`), honored by the CLI and LSP
- ✅ `sqlsift describe`: output columns and types of each query as JSON or a listing (`Analyzer::describe`, `analyzer/describe.rs`, `sqlsift-cli/src/output/describe.rs`), with parameter types (`analyzer/parameters.rs`) and NULL-ability (`analyzer/nullability.rs`, only inferred when describing); unknown names and types are null, `nullable: false` only when certain
- ✅ Multiple output formats (human, JSON, SARIF, GitHub Actions workflow commands)
- ✅ Type inference for expressions (WHERE, JOIN, INSERT VALUES, UPDATE SET, binary operators, nested expressions)
  - Detects type mismatches in comparisons (E0003)
  - Detects INSERT/UPDATE value type mismatches (E0003)
  - Detects JOIN condition type incompatibilities (E0007)
  - Supports numeric type compatibility (implicit casts)
  - Supports string-to-ENUM implicit cast
  - See "Current Limitations" for partial implementation scope

## Error Codes

- **E0001**: Table not found
- **E0002**: Column not found
- **E0003**: Type mismatch (comparisons, arithmetic, INSERT VALUES, UPDATE SET)
- **E0004**: Potential NOT NULL violation (explicit NULL assigned to a NOT NULL column)
- **E0005**: Column count mismatch (INSERT, column alias lists, subqueries, row comparisons)
- **E0006**: Ambiguous column reference
- **E0007**: JOIN type mismatch (JOIN condition type incompatibility)
- **E0008**: Missing required column (INSERT omits a NOT NULL column without DEFAULT / identity / serial / AUTO_INCREMENT)
- **E0009**: Duplicate name (table alias or CTE name given twice in one query block)
- **E0010**: Duplicate target column (INSERT column list, UPDATE SET)
- **E0011**: ORDER BY / GROUP BY position out of range
- **E0012**: Misplaced aggregate or window function (WHERE, JOIN ON, GROUP BY, nested aggregates, ...)
- **E0013**: Generated column write (INSERT/UPDATE value for a generated or GENERATED ALWAYS identity column)
- **E0014**: ON CONFLICT target matches no primary key / unique constraint / unique index
- **E0015**: SELECT DISTINCT ordered by an unselected column (and DISTINCT ON vs ORDER BY)
- **E0016**: Grouping error (column neither grouped nor aggregated; PostgreSQL/Redshift, with primary-key functional dependency)
- **E0018**-**E0023** (suspicious, warnings): comparison with NULL, NOT IN over a nullable column, IN subquery selecting an outer column, comma-joined tables with no join condition, IS NULL on a NOT NULL column / self-comparison, WHERE undoing an outer join (`analyzer/suspicious.rs`)
- **E0024**-**E0026** (restriction, off by default): UPDATE/DELETE without WHERE, INSERT without column list, SELECT * (`analyzer/restriction.rs`; only run when enabled)
- **E0017**: Value out of range (literal too long / out of range for the column in INSERT VALUES / UPDATE SET; PostgreSQL, Redshift, MySQL)
- **E0027**: Wrong argument type (built-in function or LIKE with an argument type PostgreSQL has no version for: `sum(text)`, `lower(integer)`, `int LIKE`; skipped for names the schema defines; `analyzer/signatures.rs`)
- **E0029** (suspicious, warning): type mismatch MySQL / SQLite convert at run time (what PostgreSQL rejects as E0003/E0007; `push_mismatch` in `analyzer/type_check.rs`, `SqlDialect::converts_mismatched_types`); MySQL string literals that strict mode rejects on assignment stay E0003
- **E0028** (restriction, off by default): LIMIT / OFFSET / FETCH FIRST without ORDER BY, in subqueries and CTEs too, except the query of an EXISTS (`analyzer/restriction.rs`)
- **E0030** (suspicious, warning): JOIN ... ON pairing a foreign key column with another key column instead of the column the foreign key links (`orders o JOIN users u ON u.id = o.id` for `orders.user_id REFERENCES users(id)`; `analyzer/suspicious.rs`)
- **E0031** (suspicious, warning): sum/avg/count over a table joined one-to-many next to another one-to-many join of the same parent (double counting; `analyzer/fan_out.rs`)
- **E1000**: Generic parse error

## Release Process

Releases are automated via [release-plz](https://release-plz.dev/):

1. Push commits to `main` using [Conventional Commits](https://www.conventionalcommits.org/) format (`feat:`, `fix:`, `docs:`, etc.)
2. release-plz automatically creates/updates a Release PR with version bump and CHANGELOG
3. Merge the Release PR → tag is created → `release.yml` (cargo-dist) builds and publishes

```bash
# Manual fallback (if automation fails)
./scripts/release.sh --tag <version>
```

### Configuration
- `release-plz.toml` — release-plz settings (publish, tag, semver-check)
- Only `sqlsift-cli` creates a git tag (`v{version}`) to trigger cargo-dist

- npm package: `sqlsift-cli` (provides `sqlsift` command)
- Supported platforms: macOS (x64/ARM64), Linux (x64/ARM64), Windows (x64)
- See `PUBLISHING.md` for details

### GitHub Actions: GITHUB_TOKEN Limitation (CRITICAL)

**Events created by `GITHUB_TOKEN` (releases, tag pushes, etc.) do NOT trigger other workflows.** This is GitHub's infinite loop prevention mechanism.

- cargo-dist's `release.yml` creates GitHub Releases using `GITHUB_TOKEN`, so `on: release` triggers in other workflows will NOT fire
- release-plz uses `RELEASE_PLZ_TOKEN` (a PAT) to push tags, so `on: push: tags` triggers for cargo-dist work correctly
- **When chaining workflows**: use `on: workflow_run` instead of `on: release`
  - Example: `publish-vscode.yml` uses `workflow_run: workflows: ["Release"]` to fire after cargo-dist completes
- **Always add `workflow_dispatch` alongside for manual re-runs**
