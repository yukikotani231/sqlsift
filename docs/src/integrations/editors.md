# Editors

sqlsift ships a language server, `sqlsift-lsp`, that shows diagnostics as you type. It reads `sqlsift.toml` from the workspace root, so the editor reports the same problems as the CLI, and files matched by `ignore` get no diagnostics.

## VS Code

Install the **sqlsift** extension (`sqlsift.sqlsift`) from the Marketplace. Platform builds bundle the language server, so no extra setup is required. See [`editors/vscode`](https://github.com/yukikotani231/sqlsift/tree/main/editors/vscode) for settings.

## Other editors

Install the server:

```bash
cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-lsp
```

Then register `sqlsift-lsp` (it talks LSP over stdin/stdout, with no arguments) as a language server for SQL files.

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
