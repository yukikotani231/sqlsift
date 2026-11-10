# Building the schema

`SchemaBuilder` (`schema/builder.rs`) applies DDL statements in order and produces a `Catalog` (`schema/catalog.rs`): schemas, tables with their columns, types and constraints, views, enum types, and a few settings such as the search path. The CLI builds it once per run and shares it, read-only, between the threads that check query files.

## Files in order

The schema files are applied one after the other, so the result is the database after the last migration:

- `--schema` / `schema` files are applied in the order given (a glob in the order it expands);
- `--schema-dir` / `schema_dir` takes every `.sql` file below the directory in file-name order, leaving out rollback migrations (`*.down.sql`, Flyway `U` files);
- inside a file, goose `-- +goose Down` and dbmate `-- migrate:down` sections are dropped (`schema/migrations.rs`).

Each statement changes the catalog: `CREATE TABLE` adds a table (with `LIKE`, `PARTITION OF` and `INHERITS` copying columns), `ALTER TABLE` adds, drops and renames columns and constraints, `DROP` removes, `CREATE VIEW` and `CREATE TABLE ... AS SELECT` add relations whose columns come from analyzing their query, `CREATE TYPE ... AS ENUM` adds an enum. Statements that don't change the shape of the data, such as grants and comments, are skipped. Functions, operators and triggers are only noted for what they mean to the checks: a function the schema defines is not checked against the built-in signatures, and a `BEFORE INSERT` trigger may fill `NOT NULL` columns an `INSERT` leaves out.

## When a statement can't be parsed

Real schemas contain syntax that sqlparser doesn't support yet. A file is first parsed as a whole. If that fails, it is split into statements (taking strings, comments and `$$` bodies into account) and each one is parsed on its own, so one unsupported statement only affects itself.

For a statement that still can't be parsed, the builder tries to recover what it defines:

- a `CREATE TABLE` or `CREATE VIEW` is retried without clauses that don't change its columns (storage options, `WITH NO DATA`, ...);
- if that fails too, the name of the table or view is recorded as a skipped definition.

A skipped table exists in the catalog with unknown columns. Queries on it are not reported as using a missing table, its columns are never reported as missing, and sqlsift prints one warning about the statement it skipped. Without this, a single unsupported clause in a large dump would turn every query on that table into errors.

## A catalog with history

A catalog doesn't only hold the final state. When a migration renames or drops a column or a table, the builder records the old name, what happened to it, and the file and line that did it. A query that still uses the old name gets a hint:

```text
error[E0002]: Column 'email' not found in table 'users'
    = help: 'email' was renamed to 'email_address' in migrations/0042_rename_email.sql:1
```

This is the same data `sqlsift impact` uses to explain what a migration breaks.

## Names and dialects

Tables, views and enums are keyed by schema. How an unqualified name is found depends on the dialect:

- PostgreSQL catalogs are strict: an unqualified name is looked up on the search path only (`public` unless a `SET search_path` in the schema files says otherwise), as in the database. A table that exists in another schema gets a hint to qualify it.
- MySQL and SQLite catalogs fall back to the default schema, then to any schema, since the current database is chosen when the application connects.

Identifier case follows the database: unquoted names are folded, quoted names are kept, and a column reference that only differs in case from a quoted column name is reported as not found, with a hint about the quoting.

## DDL in query files

A query file can create its own tables (`CREATE TEMP TABLE`, `CREATE TABLE ... AS`), views, or alter tables before using them. The analyzer applies those statements to a copy of the catalog made on the first such statement (`SchemaBuilder::from_catalog`), so they are visible to the later statements of that file and not to other files.

## Other sources

- dbt: `target/catalog.json` from `dbt docs generate` adds every model, seed and source with its columns, under the names that `ref()` and `source()` are rewritten to (`dbt.rs`).
- psql dumps: meta-commands such as `\connect` are blanked before parsing, so `pg_dump` output can be used as it is.
- MySQL dumps: versioned comments (`/*!50001 ... */`), `DEFINER=` clauses and similar are rewritten into something sqlparser accepts (`mysql.rs`).

`sqlsift schema` prints the catalog sqlsift built, which is the first thing to look at when a table or column is unexpectedly missing.
