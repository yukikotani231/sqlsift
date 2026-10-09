# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5](https://github.com/yukikotani231/sqlsift/compare/v0.1.4...v0.1.5) - 2026-10-09

### Added

- *(suggest)* support adjacent letter transpositions using Damerau-Levenshtein distance ([#126](https://github.com/yukikotani231/sqlsift/pull/126)) ([#151](https://github.com/yukikotani231/sqlsift/pull/151))
- sqlc query names and SQL in TypeScript tagged templates ([#115](https://github.com/yukikotani231/sqlsift/pull/115))
- mask dbt/Jinja templates in query files ([#114](https://github.com/yukikotani231/sqlsift/pull/114))
- add baseline to suppress existing diagnostics ([#113](https://github.com/yukikotani231/sqlsift/pull/113))
- file-level suppression and ignore patterns for query files ([#108](https://github.com/yukikotani231/sqlsift/pull/108))
- *(cli)* stdin input, --format github and --max-warnings ([#106](https://github.com/yukikotani231/sqlsift/pull/106))
- schema command reads config and schema dirs, shows views and enums ([#105](https://github.com/yukikotani231/sqlsift/pull/105))
- clearer table, column, enum and rule-name diagnostics ([#109](https://github.com/yukikotani231/sqlsift/pull/109))
- make tables created in a query file visible to later statements ([#103](https://github.com/yukikotani231/sqlsift/pull/103))
- skip psql meta-commands and accept psql variables ([#107](https://github.com/yukikotani231/sqlsift/pull/107))
- rule registry with categories and per-rule levels ([#80](https://github.com/yukikotani231/sqlsift/pull/80))
- check CASE branch types and MySQL inline ENUM values ([#77](https://github.com/yukikotani231/sqlsift/pull/77))
- add a GitHub Action (`uses: yukikotani231/sqlsift@...`)
- report INSERTs that omit a NOT NULL column without a default (E0008)

### Fixed

- *(baseline)* clear error message when baseline file has merge conflict markers ([#133](https://github.com/yukikotani231/sqlsift/pull/133)) ([#150](https://github.com/yukikotani231/sqlsift/pull/150))
- dbt loop separators, if/else, block tags, quoted vars, project detection and source() ([#148](https://github.com/yukikotani231/sqlsift/pull/148))
- baseline crash on non-ASCII, stale entries for gone files, partial writes and robust matching ([#146](https://github.com/yukikotani231/sqlsift/pull/146))
- COPY data blocks, schema-qualified enums, UNLOGGED/WITH NO DATA, SELECT INTO and search_path ([#147](https://github.com/yukikotani231/sqlsift/pull/147))
- mysqldump views, HAVING aliases, INSERT ... SET and index hints in MySQL ([#145](https://github.com/yukikotani231/sqlsift/pull/145))
- stack overflow on long OR chains and UTF-8 BOM parse errors ([#149](https://github.com/yukikotani231/sqlsift/pull/149))
- postgres.js helpers, fragments, Slonik tags and Vue/Svelte in embedded SQL ([#144](https://github.com/yukikotani231/sqlsift/pull/144))
- treat sqlc named parameters as placeholders ([#143](https://github.com/yukikotani231/sqlsift/pull/143))
- ignore rollback migrations when loading schema directories ([#104](https://github.com/yukikotani231/sqlsift/pull/104))
- treat views with unknown columns as unknown; allow NULL for generated integer keys
- *(schema)* apply ALTER COLUMN, DROP VIEW, ALTER TYPE, LIKE/CTAS and warn on skipped DDL
- *(types)* map FLOAT, FLOAT(p), NVARCHAR and MySQL TEXT/BLOB variants
- locate parse errors and literal type mismatches, analyze statements independently
- validate RETURNING, ON CONFLICT, enum literals and other missed clauses
- *(cli)* make JSON/SARIF output valid and fix config, color and location bugs
- resolve GROUP BY aliases, whole-row refs, system columns and multi-table DML
- follow PostgreSQL identifier case rules for table, view and CTE names
- scope set operations, USING/NATURAL and LATERAL correctly; type check every query block
- eliminate common false positives and noisy diagnostics

### Other

- add a roadmap for v0.2, v0.3 and beyond ([#163](https://github.com/yukikotani231/sqlsift/pull/163))
- enable clippy pedantic lints and drop unneeded clones ([#111](https://github.com/yukikotani231/sqlsift/pull/111))
- add an mdBook user guide and slim down the README ([#112](https://github.com/yukikotani231/sqlsift/pull/112))
- add CONTRIBUTING.md ([#110](https://github.com/yukikotani231/sqlsift/pull/110))
- CI recipe for re-checking queries when the schema changes ([#102](https://github.com/yukikotani231/sqlsift/pull/102))
- unify name resolution and type checking in one scoped walk ([#79](https://github.com/yukikotani231/sqlsift/pull/79))
- linear-time recovery from syntax errors, parallel file analysis ([#78](https://github.com/yukikotani231/sqlsift/pull/78))
- Merge remote-tracking branch 'origin/main' into feat/github-action
- link the browser playground from the README
- Merge remote-tracking branch 'origin/main' into fix/cli-lsp
- Merge remote-tracking branch 'origin/main' into test/coverage
- update README example output to the aligned human format
- Merge remote-tracking branch 'origin/main' into fix/cli-lsp
- Merge remote-tracking branch 'origin/main' into docs/readme-revamp
- update coverage expectations for table suggestions and boolean literals
- add schema building and diagnostic quality tests
- add MySQL and SQLite dialect coverage tests
- add PostgreSQL query coverage tests
- fix clippy lints reported by current stable toolchain
- rework README to lead with a real example and comparison

## [0.1.4](https://github.com/yukikotani231/sqlsift/compare/v0.1.3...v0.1.4) - 2026-03-04

### Added

- implement E0004 null-violation diagnostics ([#66](https://github.com/yukikotani231/sqlsift/pull/66))

### Added
- Implement `E0004` potential null-violation diagnostics for explicit `NULL` assignment to `NOT NULL` columns in `INSERT` / `UPDATE`
- Add analyzer tests for `E0004` positive/negative cases

## [0.1.3](https://github.com/yukikotani231/sqlsift/compare/v0.1.2...v0.1.3) - 2026-03-04

### Added

- validate set operations and wire CLI runtime flags ([#65](https://github.com/yukikotani231/sqlsift/pull/65))

### Other

- refresh outdated capability notes ([#63](https://github.com/yukikotani231/sqlsift/pull/63))

### Added
- Add set operation type checks for `UNION` / `INTERSECT` / `EXCEPT`
  - Detect column count mismatch between left/right projections
  - Detect per-column type incompatibility in set operations
- Add analyzer tests for set operation mismatch/compatible/wildcard scenarios

## [0.1.2](https://github.com/yukikotani231/sqlsift/compare/v0.1.1...v0.1.2) - 2026-02-19

### Added

- support DROP TABLE to keep schema catalog in sync ([#55](https://github.com/yukikotani231/sqlsift/pull/55))
- add inline comment directives for diagnostic suppression ([#52](https://github.com/yukikotani231/sqlsift/pull/52))

### Fixed

- isolate subquery scope to prevent false E0006 ambiguity ([#60](https://github.com/yukikotani231/sqlsift/pull/60))
- support INSERT/UPDATE RETURNING columns in CTE inference ([#59](https://github.com/yukikotani231/sqlsift/pull/59))

### Other

- add GitHub templates and CI integration examples ([#54](https://github.com/yukikotani231/sqlsift/pull/54))

## [0.1.1](https://github.com/yukikotani231/sqlsift/compare/sqlsift-core-v0.1.0...sqlsift-core-v0.1.1) - 2026-02-14

### Added

- add function return type inference for type checking ([#39](https://github.com/yukikotani231/sqlsift/pull/39))
- add CAST expression type inference
- add SQLite dialect support
- add INSERT VALUES and UPDATE SET type checking (E0003)

### Other

- fix assert_eq formatting for nightly rustfmt
- fix formatting for nightly rustfmt
- update README and CLAUDE.md for current state
- Rename project from sqlsurge to sqlsift
- add comprehensive TODO documentation for type inference
- Prepare v0.1.0-alpha.5 release
- Update docs to reflect current PostgreSQL support level
- Update README and CLAUDE.md to reflect current features
- Prepare v0.1.0-alpha.1 release
- Add npm package distribution via cargo-dist
- Fix GitHub username in URLs
- Add README, CLAUDE.md, and CI workflow
