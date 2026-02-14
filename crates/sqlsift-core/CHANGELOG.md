# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
