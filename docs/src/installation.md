# Installation

## npm

Prebuilt binaries for macOS (x64, ARM64), Linux (x64, ARM64) and Windows (x64) are published to npm as `sqlsift-cli`, which provides the `sqlsift` command:

```bash
npm install -g sqlsift-cli
sqlsift --version
```

Or run it without installing:

```bash
npx sqlsift-cli check --schema schema.sql queries/*.sql
```

In a Node project you can add it as a dev dependency (`npm install --save-dev sqlsift-cli`) and call `sqlsift` from your `package.json` scripts.

## Prebuilt binaries

Archives for every platform are attached to each [GitHub release](https://github.com/yukikotani231/sqlsift/releases).

## From source

With a [Rust toolchain](https://rustup.rs/) installed:

```bash
cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-cli
```

The language server for editors is a separate binary:

```bash
cargo install --git https://github.com/yukikotani231/sqlsift sqlsift-lsp
```

## Try it without installing

The [playground](https://yukikotani231.github.io/sqlsift/) runs sqlsift in your browser.
