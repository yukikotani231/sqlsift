# sqlsift VS Code Extension

SQL static analysis extension powered by [sqlsift](https://github.com/yukikotani231/sqlsift). Validates SQL queries against schema definitions and shows diagnostics in real-time.

## Checked files

- SQL files (`sql` language)
- dbt models: `jinja-sql` documents (set by the "dbt Power User" extension) and `.sql` files in the `jinja` language. Jinja is also masked in any SQL file inside a dbt project (a `dbt_project.yml` in one of its parent directories).
- TypeScript and JavaScript files: the SQL in tagged template literals whose tag is in `embedded_sql_tags` in `sqlsift.toml` (default `["sql"]`). Files without such a template get no diagnostics. Turn this off with `sqlsift.embeddedSql.enable`.

## Prerequisites

`sqlsift-lsp` binary must be available in your PATH.

```bash
# From the repository root
cargo install --path crates/sqlsift-lsp
```

## Installation

### From .vsix file

```bash
# Build the .vsix package
cd editors/vscode
npm install
npm run compile
npx @vscode/vsce package --allow-missing-repository

# Install in VS Code
code --install-extension sqlsift-0.1.0.vsix
```

### Development (Extension Development Host)

```bash
cd editors/vscode
npm install
npm run compile
```

Then open `editors/vscode/` in VS Code and press F5.

## Setup

Create a `sqlsift.toml` in your project root:

```toml
# Schema file paths (glob patterns supported)
schema = ["db/schema.sql"]

# Or specify a directory (recursively finds *.sql)
# schema_dir = "db/migrations"

# SQL dialect: "postgresql" (default), "mysql" or "sqlite"
# dialect = "postgresql"

# Template literal tags checked as SQL in TypeScript / JavaScript files
# embedded_sql_tags = ["sql", "$queryRaw"]

# Disable specific rules
# disable = ["E0001"]

# Rule levels: "off", "warn" or "error" (by code or name)
# [rules]
# ambiguous-column = "warn"
```

## Extension Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `sqlsift.serverPath` | `sqlsift-lsp` | Path to the sqlsift-lsp binary |
| `sqlsift.embeddedSql.enable` | `true` | Check SQL in tagged template literals (`` sql`...` ``) of TypeScript and JavaScript files (reload the window after changing it) |

## Uninstall

```bash
code --uninstall-extension sqlsift.sqlsift
```
