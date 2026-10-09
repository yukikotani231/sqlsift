# Editors

sqlsift ships a language server, `sqlsift-lsp`, that shows diagnostics as you type. It reads `sqlsift.toml` from the workspace root, so the editor reports the same problems as the CLI, and files matched by `ignore` get no diagnostics.

Besides `.sql` files, the server checks:

- **TypeScript and JavaScript** (`typescript`, `typescriptreact`, `javascript`, `javascriptreact` documents, or `.ts`, `.tsx`, `.js`, `.jsx`, `.mts`, `.cts` files): the SQL in tagged template literals whose tag is in `embedded_sql_tags` (default `["sql"]`), as `sqlsift check` does. Files without such a template get no diagnostics, and hover and completion are only offered in SQL documents.
- **dbt models**: Jinja templates are masked in `jinja-sql` documents (the language the "dbt Power User" extension sets) and in any document with a `dbt_project.yml` in one of its parent directories, so a dbt project in a subdirectory of the workspace works too. An explicit `templating` in `sqlsift.toml` applies to every document.

## VS Code

Install the **sqlsift** extension (`sqlsift.sqlsift`) from the Marketplace. Platform builds bundle the language server, so no extra setup is required. See [`editors/vscode`](https://github.com/yukikotani231/sqlsift/tree/main/editors/vscode) for settings. It activates on SQL, `jinja-sql`, Jinja `.sql` files and, unless `sqlsift.embeddedSql.enable` is `false`, TypeScript and JavaScript files.

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
