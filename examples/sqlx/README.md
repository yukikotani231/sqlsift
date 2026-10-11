# sqlx (Rust, PostgreSQL)

A Rust service using [sqlx](https://github.com/launchbadge/sqlx) with its SQL in files and inline: `query_file!` / `query_file_as!` load `queries/*.sql` at compile time, `src/admin.rs` writes its SQL in `query_as!(...)` and `sqlx::query_scalar(...)`, and `migrate!()` applies `migrations/<timestamp>_<name>.sql`. sqlsift checks all of it against the migrations, with no database and no `DATABASE_URL`.

```text
Cargo.toml
sqlsift.toml
src/main.rs                                                   query_file!("queries/open_todos.sql", owner), ...
src/admin.rs                                                  query_as!(Overdue, r#"SELECT ..."#), sqlx::query_scalar("SELECT ...")
migrations/20260301090000_create_users.sql
migrations/20260305140000_create_todos.sql
migrations/20260320110000_replace_done_with_completed_at.sql  drops todos.done
queries/*.sql                                                 one statement per file, $1 parameters
broken/*.sql, broken/admin.rs                                 intentional mistakes
```

`sqlsift.toml` (what `sqlsift init` writes for this layout, minus `broken/`):

```toml
schema_dir = "migrations"
files = ["queries/**/*.sql", "src/**/*.rs"]
dialect = "postgresql"
```

The migrations are applied in file name order, so the timestamp prefix gives sqlx's order; the `*.down.sql` halves of reversible migrations are skipped. sqlx's type overrides in column aliases (`id AS "id!"`, `priority AS "priority: Priority"`) are ordinary quoted aliases to sqlsift. In `.rs` files, sqlsift checks the string literals passed to `query!`, `query_as!`, `query_scalar!` (and their `_unchecked` variants) and to the `sqlx::query`, `sqlx::query_as` and `sqlx::query_scalar` functions; diagnostics point into the Rust file.

sqlx already checks the macros' queries when it compiles, against a live database or the `.sqlx` cache from `cargo sqlx prepare`, and the functions' queries only when they run. sqlsift checks both in a second, needs neither, and runs where the Rust build doesn't: a pull request that only adds a migration, an editor, a pre-commit hook. The Rust code here is for illustration and isn't built in sqlsift's CI.

## Run it

```console
$ sqlsift check
All 8 file(s) passed validation
```

## What it catches

`broken/` has four mistakes. `overdue_todos.sql` was written before the last migration dropped `todos.done`; with `SQLX_OFFLINE=true` and a stale `.sqlx` cache it still compiles. `admin.rs` makes the same mistake in a `sqlx::query_scalar` call, which compiles whatever its SQL says:

```console
$ sqlsift check broken/*
error[E0002]: Column 'done' not found in table 'todos'
  --> broken/admin.rs:8:80
    |
  8 |     sqlx::query_scalar("SELECT count(*) FROM todos WHERE owner_id = $1 AND NOT done")
    |                                                                                ^^^^
    = help: 'done' was dropped in migrations/20260320110000_replace_done_with_completed_at.sql:4

error[E0008]: INSERT into 'todos' is missing required column 'owner_id'
  --> broken/create_todo.sql:1:13
    |
  1 | INSERT INTO todos (title, priority)
    |             ^^^^^
    = help: 'owner_id' is NOT NULL without a default. Provide a value, or add a DEFAULT to the schema

error[E0002]: Column 'done' not found in table 'todos'
  --> broken/overdue_todos.sql:7:11
    |
  7 |   AND NOT done
    |           ^^^^
    = help: 'done' was dropped in migrations/20260320110000_replace_done_with_completed_at.sql:4

error[E0003]: Invalid value 'urgent' for enum type 'todo_priority'
  --> broken/set_priority.sql:2:5
    |
  2 | SET priority = 'urgent'
    |     ^^^^^^^^
    = help: Valid values: 'low', 'normal', 'high'

Found 4 error(s), 0 warning(s) in 4 file(s)
```

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
