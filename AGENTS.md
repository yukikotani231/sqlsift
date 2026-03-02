# Repository Guidelines

## Project Structure & Module Organization
This repository is a Rust workspace for `sqlsift`, with three primary crates under `crates/`:
- `sqlsift-core`: SQL schema catalog + static analysis engine.
- `sqlsift-cli`: command-line interface (`sqlsift`).
- `sqlsift-lsp`: Language Server Protocol backend.

VS Code integration lives in `editors/vscode/` (TypeScript client extension). SQL test inputs are in `tests/fixtures/` and `tests/fixtures/real-world/`. Release tooling and automation configs are at the root (`release-plz.toml`, `dist-workspace.toml`, `scripts/release.sh`).

## Build, Test, and Development Commands
- `cargo build --all-targets`: Build all crates and targets in the workspace.
- `cargo test --all-targets`: Run unit, integration, and doc tests.
- `cargo fmt --all -- --check`: Verify formatting (CI-enforced).
- `cargo clippy --all-targets -- -D warnings`: Lint with warnings treated as errors.
- `cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/valid_query.sql`: Run CLI on sample inputs.

Optional local hook setup:
- `git config core.hooksPath .githooks` enables pre-commit checks for `fmt` and `clippy`.

## Coding Style & Naming Conventions
Use standard Rust style (4-space indentation, `rustfmt` output as source of truth). Keep modules focused and domain-oriented (`schema`, `analyzer`, `types`, `output`). Use `snake_case` for functions/modules/files, `CamelCase` for types/traits, and `SCREAMING_SNAKE_CASE` for constants. Prefer explicit error propagation over `unwrap()` in library code.

For VS Code extension code, follow TypeScript conventions already used in `editors/vscode/src/extension.ts`.

## Testing Guidelines
Place unit tests near implementation (`#[cfg(test)]`), and use fixture-driven integration tests for SQL behavior. Add both positive and negative cases when introducing diagnostics (valid SQL and expected errors). Keep fixture names descriptive, e.g. `feature-name-queries.sql` and `feature-name-invalid-queries.sql`.

## Commit & Pull Request Guidelines
Follow Conventional Commits, e.g. `feat: ...`, `fix: ...`, `docs: ...`, with `!` for breaking changes. Keep commits scoped and buildable.

PRs should include:
- A concise summary of behavior changes.
- Linked issue(s) when applicable.
- Passing checks for `cargo test` and `cargo clippy --all-targets -- -D warnings`.
- Updated tests/docs for user-visible changes.
