# Contributing to sqlsift

Thanks for your interest in sqlsift! Bug reports, real-world SQL samples and code are all welcome.

## Reporting problems

The most valuable contribution right now is **SQL that sqlsift gets wrong**:

- **False positive**: valid SQL that sqlsift reports as an error.
- **Missed error**: broken SQL that sqlsift accepts.
- **Parse failure**: valid SQL for your dialect that sqlsift can't parse.

Please [open an issue](https://github.com/yukikotani231/sqlsift/issues/new/choose) with:

1. A minimal schema (`CREATE TABLE ...`) and query that reproduce it. The [playground](https://yukikotani231.github.io/sqlsift/) is a quick way to cut it down.
2. The dialect (`postgresql`, `mysql` or `sqlite`) and the sqlsift version (`sqlsift --version`).
3. What you expected and what sqlsift reported.

Feature ideas go in a [feature request](https://github.com/yukikotani231/sqlsift/issues/new/choose). For larger changes, please open an issue to discuss the approach before sending a PR.

## Development setup

You need a stable [Rust toolchain](https://rustup.rs/). Node.js is only needed for the VS Code extension.

```bash
git clone https://github.com/yukikotani231/sqlsift && cd sqlsift
git config core.hooksPath .githooks   # run fmt + clippy before each commit
cargo build
cargo test
```

Try the CLI on the bundled fixtures:

```bash
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/valid_query.sql
cargo run -- check --schema tests/fixtures/schema.sql tests/fixtures/invalid_query.sql
```

Before opening a PR, make sure these pass (CI runs the same checks):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## Repository layout

| Path | What it is |
| --- | --- |
| `crates/sqlsift-core` | Schema catalog, DDL parsing and the analysis engine |
| `crates/sqlsift-cli` | The `sqlsift` command (args, config file, output formats) |
| `crates/sqlsift-lsp` | Language server used by editor integrations |
| `crates/sqlsift-wasm` | WebAssembly bindings for the playground |
| `editors/vscode` | VS Code extension (LSP client) |
| `site` | Browser playground, deployed to GitHub Pages |
| `docs` | User guide ([mdBook](https://rust-lang.github.io/mdBook/)), deployed to GitHub Pages under `/docs/` |
| `tests/fixtures` | SQL fixtures, including real-world schemas (Chinook, Pagila, Northwind) |

The flow is: schema SQL is parsed by [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs) and turned into an in-memory `Catalog` by `SchemaBuilder`; each query is then walked once by the `Resolver`, which resolves names through `Scope` and infers types, emitting diagnostics. [`CLAUDE.md`](CLAUDE.md) has a more detailed architecture overview and the current list of limitations.

## Common changes

### Fixing a false positive or missed error

1. Add a failing test in `crates/sqlsift-core/tests/` that reproduces the issue (regressions usually go in `regression_tests.rs`).
2. Fix it in `crates/sqlsift-core/src/analyzer/` (name resolution in `resolver.rs`, types in `type_check.rs`) or `schema/builder.rs` (DDL).
3. Cover both sides: a valid query that must stay clean and an invalid one that must still be reported.

### Adding a diagnostic rule

1. Add a variant to `DiagnosticKind` in `crates/sqlsift-core/src/error.rs`, and its entry (code, name, category, summary) to `RULES` in `rules.rs` in the same position.
2. Pick the category by how sure the rule is: `correctness` (definitely wrong, error by default), `suspicious` (likely wrong, warning by default), or `pedantic` / `style` / `restriction` (opt-in).
3. Implement detection in `analyzer/resolver.rs` or `analyzer/type_check.rs`. Look names up through `Scope` rather than walking FROM clauses yourself.
4. Add tests, add the rule to the rule table in the README and `docs/src/rules/index.md`, and add a page for it in `docs/src/rules/` (listed in `docs/src/SUMMARY.md`).

### Adding a CLI or config option

Add the flag in `crates/sqlsift-cli/src/args.rs`, the matching field in `config.rs` (with `#[serde(default)]` and an entry in `KNOWN_KEYS`), merge it in `Config::merge_with_args()`, and document it in the sample `sqlsift.toml` and the user guide (`docs/src/reference/`).

### Working on the VS Code extension

```bash
cargo build -p sqlsift-lsp
cd editors/vscode && npm ci && npm run compile
```

Then open `editors/vscode` in VS Code and press F5 to launch an Extension Development Host.

### Working on the user guide

The guide is an [mdBook](https://rust-lang.github.io/mdBook/) in `docs/`. Install it with `cargo install mdbook` and run `mdbook serve docs --open` to preview changes as you edit.

### Working on the playground

`scripts/build-playground.sh` builds the wasm bundle into `site/` (it needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli`). Serve `site/` with any static file server to try it locally.

## Pull requests

- Keep each PR focused on one change, with tests for user-visible behavior.
- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages and PR titles (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`; add `!` for breaking changes). Releases and the changelog are generated from them by [release-plz](https://release-plz.dev/), so don't edit version numbers or `CHANGELOG.md` by hand.
- Update the user guide in `docs/` (and the README or the sample `sqlsift.toml` where relevant) when behavior or options change.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
