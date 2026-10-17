# Editors

sqlsift ships a language server, `sqlsift-lsp`, that shows diagnostics as you type. It reads `sqlsift.toml` from the workspace root, so the editor reports the same problems as the CLI, and files matched by `ignore` get no diagnostics.

Besides `.sql` files, the server checks:

- **TypeScript and JavaScript** (`typescript`, `typescriptreact`, `javascript`, `javascriptreact` documents, or `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts` files): the SQL in tagged template literals whose tag is in `embedded_sql_tags` (default `["sql"]`), as `sqlsift check` does. Files without such a template get no diagnostics, and hover and completion are only offered in SQL documents.
- **Rust** (`rust` documents, or `.rs` files): the SQL passed to sqlx's query macros and `sqlx::` query functions named in `embedded_sql_macros`, as `sqlsift check` does (see [SQL in Rust (sqlx)](../guide/queries.md#sql-in-rust-sqlx)). Files without such a query get no diagnostics.
- **Python** (`python` documents, or `.py` files): the SQL passed as a string literal to the calls named in `embedded_sql_functions` (`cur.execute("...")`, `text("...")`, ...), as `sqlsift check` does (see [SQL in Python](../guide/queries.md#sql-in-python)). Files without such a call get no diagnostics.
- **Go** (`go` documents, or `.go` files): the SQL passed as a string literal to the `database/sql`, sqlx and pgx calls named in `embedded_sql_go_functions` (`db.QueryContext(ctx, "...")`, ...), as `sqlsift check` does (see [SQL in Go](../guide/queries.md#sql-in-go)). Files without such a call get no diagnostics.
- **dbt models**: Jinja templates are masked in `jinja-sql` documents (the language the "dbt Power User" extension sets) and in any document with a `dbt_project.yml` in one of its parent directories, so a dbt project in a subdirectory of the workspace works too. An explicit `templating` in `sqlsift.toml` applies to every document.

## VS Code

Install the **sqlsift** extension (`sqlsift.sqlsift`) from the Marketplace. Platform builds bundle the language server, so no extra setup is required. See [`editors/vscode`](https://github.com/yukikotani231/sqlsift/tree/main/editors/vscode) for settings. It activates on SQL, `jinja-sql`, Jinja `.sql` files and, unless `sqlsift.embeddedSql.enable` is `false`, TypeScript, JavaScript, Rust, Python and Go files.

## Other editors

Install the server:

```bash
cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-lsp
```

Then register `sqlsift-lsp` (it talks LSP over stdin/stdout, with no arguments) as a language server for SQL files (and, to check embedded SQL, for TypeScript and JavaScript files).

### Neovim (nvim-lspconfig)

```lua
vim.lsp.config('sqlsift', {
  cmd = { 'sqlsift-lsp' },
  filetypes = { 'sql' },
  root_markers = { 'sqlsift.toml', '.git' },
})
vim.lsp.enable('sqlsift')
```

### Helix

```toml
# ~/.config/helix/languages.toml
[language-server.sqlsift]
command = "sqlsift-lsp"

[[language]]
name = "sql"
language-servers = ["sqlsift"]
```

## Quick fixes

When your editor supports LSP code actions (e.g. VS Code's lightbulb, Neovim's `vim.lsp.buf.code_action()`), `sqlsift-lsp` offers quick fixes for diagnostics in `.sql` files:
- **Disable `<RULE>` for this line**: inserts `-- sqlsift:disable <RULE>` above the diagnostic, indented to match the line.
- **Disable `<RULE>` for this file**: inserts `-- sqlsift:disable-file <RULE>` as the first line of the file.

