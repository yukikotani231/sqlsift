# sqlsurge VS Code Extension

SQL static analysis extension powered by [sqlsurge](https://github.com/yukikotani231/sqlsurge). Validates SQL queries against schema definitions and shows diagnostics in real-time.

## Prerequisites

`sqlsurge-lsp` binary must be available in your PATH.

```bash
# From the repository root
cargo install --path crates/sqlsurge-lsp
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
code --install-extension sqlsurge-0.1.0.vsix
```

### Development (Extension Development Host)

```bash
cd editors/vscode
npm install
npm run compile
```

Then open `editors/vscode/` in VS Code and press F5.

## Setup

Create a `sqlsurge.toml` in your project root:

```toml
# Schema file paths (glob patterns supported)
schema = ["db/schema.sql"]

# Or specify a directory (recursively finds *.sql)
# schema_dir = "db/migrations"

# SQL dialect: "postgresql" (default) or "mysql"
# dialect = "postgresql"

# Disable specific rules
# disable = ["E0001"]
```

## Extension Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `sqlsurge.serverPath` | `sqlsurge-lsp` | Path to the sqlsurge-lsp binary |

## Uninstall

```bash
code --uninstall-extension sqlsurge.sqlsurge
```
