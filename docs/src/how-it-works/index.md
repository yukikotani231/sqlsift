# How sqlsift works

This section explains what happens between `sqlsift check` and the diagnostics it prints: how the schema becomes an in-memory catalog, how a query is checked against it, and how the project makes sure valid SQL isn't reported. It is meant for contributors and for anyone who wants to know why they can trust the results. You don't need it to use sqlsift.

- [Name resolution and types](names-and-types.md): how a query is walked, which names are visible where, and how types are inferred
- [Building the schema](schema.md): from DDL files and migrations to a `Catalog`
- [Preprocessing and locations](preprocessing.md): checking psql scripts, dbt models and SQL embedded in TypeScript, Python, Go and Rust with a parser that only reads SQL
- [Testing and the real-world corpus](testing.md): how false positives and missed errors are measured

The source is the reference; these pages name the modules and functions to look for.

## The pipeline

```text
schema files ─▶ sqlparser ─▶ SchemaBuilder ─▶ Catalog (tables, columns, types, constraints, views, enums)
                                                │
query files                                     ▼
  ─▶ preprocessing (templates, psql, parameters, embedded SQL; lines and columns kept)
  ─▶ sqlparser ─▶ Resolver: one walk per statement, names and types together ─▶ diagnostics
                                                │
                     suppression comments, rule levels, baseline
                                                ▼
                     CLI output · language server · WebAssembly playground · GitHub Action
```

The code is a Cargo workspace:

| Crate | Role |
|---|---|
| `sqlsift-core` | Everything that matters: schema building, analysis, rules, baseline, `impact` / `describe` / `unused` |
| `sqlsift-cli` | The `sqlsift` command: configuration, finding files, running in parallel, output formats |
| `sqlsift-lsp` | The language server used by the VS Code extension |
| `sqlsift-wasm` | The bindings behind the [playground](https://sqlsift.dev/) |

`sqlsift-core` does no I/O. It takes text and returns diagnostics, which is why the CLI, the editor, the browser and CI give the same results.

## Two principles

Most design decisions follow from two rules.

**One walk per statement.** The `Resolver` (`analyzer/resolver.rs`) visits each query block once. While it resolves names, it infers types, runs the checks and returns the block's output columns with their types. A CTE, a subquery or a view hands its typed columns to the query that uses it, so nothing is walked twice for type checking. The checks themselves live in separate files (`type_check.rs`, `structure.rs`, `suspicious.rs`, ...) but all run inside this walk. The `describe`, `impact` and `unused` commands reuse it too, by recording a little more as it goes (output columns, parameter types, references to tables and columns).

**When in doubt, say nothing.** A static analyzer is only useful if people can trust it, and one false positive costs more trust than one missed error. So anything sqlsift can't be sure of is left unreported:

- a type that can't be inferred is `Unknown` and is never compared;
- a table whose `CREATE TABLE` couldn't be parsed exists, with unknown columns;
- a column that may come from a relation of unknown shape is not reported as missing;
- a name that a template or a string interpolation substitutes is not checked;
- rules that are often right but not always (comparisons with `NULL`, joins that multiply rows) are warnings, not errors.

## Rules come last

Checks don't know whether their rule is enabled. They push a `Diagnostic` with its `DiagnosticKind`, and at the end of a file `RuleConfig::apply` (`rules.rs`) drops the diagnostics of rules that are off and sets the severity of the others. A rule's level comes from its own setting, else its category's, else the category default. The rule registry (`RULES`) is an array in the same order as the `DiagnosticKind` variants, so looking a rule up is an index.

The only exception is the restriction category (`SELECT *`, `UPDATE` without `WHERE`, ...): those rules are off by default, so their checks only run when one of them is enabled.

Before the levels are applied, `-- sqlsift:disable` comments remove diagnostics by line. After them, the CLI and the language server filter out what the [baseline](../guide/suppression.md) already knows. Baseline entries hold no line numbers: they match on the rule and a hash of the normalized statement, so adding lines or reformatting a query doesn't break the match.
