# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/yukikotani231/sqlsift/compare/v0.2.0...v0.2.1) - 2026-10-17

### Added

- add --format option to sqlsift rules ([#386](https://github.com/yukikotani231/sqlsift/pull/386))
- report duplicate primary key and unique values in INSERT VALUES list (E0035) ([#378](https://github.com/yukikotani231/sqlsift/pull/378))
- report CASE over enum column missing labels (E0034) ([#377](https://github.com/yukikotani231/sqlsift/pull/377))
- report timestamp vs timestamptz comparisons and assignments (E0033) ([#376](https://github.com/yukikotani231/sqlsift/pull/376))
- E0032 integer-division warns when integer columns are divided ([#375](https://github.com/yukikotani231/sqlsift/pull/375))
- check SQL in Go code (database/sql, sqlx and pgx calls) ([#315](https://github.com/yukikotani231/sqlsift/pull/315))
- E0031 join-fan-out warns on aggregates multiplied by another one-to-many join ([#310](https://github.com/yukikotani231/sqlsift/pull/310))
- E0030 foreign-key-join-mismatch warns on joins that don't follow the foreign key ([#308](https://github.com/yukikotani231/sqlsift/pull/308))
- sqlsift unused lists tables and columns no query uses ([#306](https://github.com/yukikotani231/sqlsift/pull/306))
- check SQL in Python database calls ([#301](https://github.com/yukikotani231/sqlsift/pull/301))
- *(baseline)* strict mode, pruning, per-rule rewrite, stats and SARIF suppressions ([#285](https://github.com/yukikotani231/sqlsift/pull/285))
- check SQL in Rust sqlx query macros and functions ([#278](https://github.com/yukikotani231/sqlsift/pull/278))
- *(action)* `mode: impact` reports what the changed migrations break ([#277](https://github.com/yukikotani231/sqlsift/pull/277))
- *(describe)* infer parameter types and output column NULL-ability ([#273](https://github.com/yukikotani231/sqlsift/pull/273))
- add sqlsift describe (output columns and types of each query) ([#269](https://github.com/yukikotani231/sqlsift/pull/269))
- record the uses of catalog tables and columns; `sqlsift impact --references` ([#272](https://github.com/yukikotani231/sqlsift/pull/272))
- *(cli)* `sqlsift impact --base <rev>` picks the schema files changed on the branch ([#270](https://github.com/yukikotani231/sqlsift/pull/270))
- *(cli)* add `sqlsift impact` to show the queries a schema change breaks or fixes ([#268](https://github.com/yukikotani231/sqlsift/pull/268))
- *(analyzer)* limit-without-order-by restriction rule (E0028)

### Fixed

- raise parser recursion limit for deeply nested subqueries ([#384](https://github.com/yukikotani231/sqlsift/pull/384))
- *(cli)* use merge base content for in-place edited schema files in impact --base ([#382](https://github.com/yukikotani231/sqlsift/pull/382))
- sort schema-dir migrations by natural version order ([#367](https://github.com/yukikotani231/sqlsift/pull/367))
- report type mismatches MySQL and SQLite convert at run time as E0029 warnings ([#305](https://github.com/yukikotani231/sqlsift/pull/305))
- *(init)* pick the schema source with the most tables and the project's database ([#304](https://github.com/yukikotani231/sqlsift/pull/304))
- read UTF-16 files and skip undecodable query files with a warning ([#303](https://github.com/yukikotani231/sqlsift/pull/303))

### Other

- describe schema-dir version ordering ([#371](https://github.com/yukikotani231/sqlsift/pull/371))
- mention SQL in Go code and the Go example in README ([#317](https://github.com/yukikotani231/sqlsift/pull/317))
- use mimalloc as global allocator in CLI and LSP ([#243](https://github.com/yukikotani231/sqlsift/pull/243))
- Merge pull request #259 from cesartalen/feat/limit-without-order-by
- Merge remote-tracking branch 'origin/main' into feat/limit-without-order-by
- link the action's Marketplace listing and add the social preview asset ([#262](https://github.com/yukikotani231/sqlsift/pull/262))
- move the playground and user guide links to sqlsift.dev ([#261](https://github.com/yukikotani231/sqlsift/pull/261))
- lead the README with the schema-aware punchline and a demo GIF ([#260](https://github.com/yukikotani231/sqlsift/pull/260))
- update roadmap after v0.2 and plan the in-house PostgreSQL parser for v0.4 ([#258](https://github.com/yukikotani231/sqlsift/pull/258))
- release v0.2.0 ([#189](https://github.com/yukikotani231/sqlsift/pull/189))

## [0.2.0](https://github.com/yukikotani231/sqlsift/compare/v0.1.5...v0.2.0) - 2026-10-11

### Added

- aiosql / HugSQL query names and parameters, with an example project ([#236](https://github.com/yukikotani231/sqlsift/pull/236))
- *(analyzer)* name the migration file and line in rename/drop hints ([#235](https://github.com/yukikotani231/sqlsift/pull/235))
- *(analyzer)* wrong-argument-type rule for built-in functions and LIKE (E0027) ([#233](https://github.com/yukikotani231/sqlsift/pull/233))
- *(cli)* sqlsift init detects the project layout and writes sqlsift.toml ([#225](https://github.com/yukikotani231/sqlsift/pull/225))
- *(action)* report only the diagnostics a pull request introduces ([#223](https://github.com/yukikotani231/sqlsift/pull/223))
- *(analyzer)* value-out-of-range, suspicious and restriction rules E0017-E0026 ([#222](https://github.com/yukikotani231/sqlsift/pull/222))
- *(analyzer)* add structural and grouping rules E0009-E0016 ([#221](https://github.com/yukikotani231/sqlsift/pull/221))
- *(dbt)* resolve ref() / source() columns from dbt's catalog.json (alpha) ([#190](https://github.com/yukikotani231/sqlsift/pull/190))
- *(dialect)* add Snowflake, BigQuery, Redshift and Databricks dialects (alpha) ([#188](https://github.com/yukikotani231/sqlsift/pull/188))

### Fixed

- skip directories matched by globs; PostgreSQL case-sensitive quoted columns ([#228](https://github.com/yukikotani231/sqlsift/pull/228))
- *(dbt)* parse config after comments, clause macros, loop aliases and var() relations ([#197](https://github.com/yukikotani231/sqlsift/pull/197))

### Other

- sqlx example project and a sweep of stale statements ([#234](https://github.com/yukikotani231/sqlsift/pull/234))
- add example projects for common stacks, checked in CI ([#224](https://github.com/yukikotani231/sqlsift/pull/224))

## [0.1.5](https://github.com/yukikotani231/sqlsift/compare/v0.1.4...v0.1.5) - 2026-10-11

### Added

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
- stack overflow on long OR chains and UTF-8 BOM parse errors ([#149](https://github.com/yukikotani231/sqlsift/pull/149))
- postgres.js helpers, fragments, Slonik tags and Vue/Svelte in embedded SQL ([#144](https://github.com/yukikotani231/sqlsift/pull/144))
- treat sqlc named parameters as placeholders ([#143](https://github.com/yukikotani231/sqlsift/pull/143))
- ignore rollback migrations when loading schema directories ([#104](https://github.com/yukikotani231/sqlsift/pull/104))
- *(cli)* use '/' in SARIF URIs; make path assertions separator-agnostic
- *(cli)* avoid bail! in expression position; make stderr assertions wrap-tolerant
- *(cli)* make JSON/SARIF output valid and fix config, color and location bugs
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
- update README example output to the aligned human format
- Merge remote-tracking branch 'origin/main' into fix/cli-lsp
- Merge remote-tracking branch 'origin/main' into docs/readme-revamp
- add CLI and LSP end-to-end coverage tests
- rework README to lead with a real example and comparison

## [0.1.4](https://github.com/yukikotani231/sqlsift/compare/v0.1.3...v0.1.4) - 2026-03-04

### Added

- implement E0004 null-violation diagnostics ([#66](https://github.com/yukikotani231/sqlsift/pull/66))

## [0.1.3](https://github.com/yukikotani231/sqlsift/compare/v0.1.2...v0.1.3) - 2026-03-04

### Added

- validate set operations and wire CLI runtime flags ([#65](https://github.com/yukikotani231/sqlsift/pull/65))

### Other

- refresh outdated capability notes ([#63](https://github.com/yukikotani231/sqlsift/pull/63))

### Changed
- Implement `--max-errors` early-stop behavior in `check`
- Implement `-q/--quiet` summary suppression behavior
- Implement `-v/--verbose` logging level behavior (`-vv` enables debug)

### Added
- Add CLI integration tests for `--max-errors`, `-q`, and `-v`

## [0.1.2](https://github.com/yukikotani231/sqlsift/compare/v0.1.1...v0.1.2) - 2026-02-19

### Added

- add inline comment directives for diagnostic suppression ([#52](https://github.com/yukikotani231/sqlsift/pull/52))

### Other

- add GitHub templates and CI integration examples ([#54](https://github.com/yukikotani231/sqlsift/pull/54))

## [0.1.1](https://github.com/yukikotani231/sqlsift/compare/v0.1.0...v0.1.1) - 2026-02-14

### Other

- update Cargo.lock dependencies
