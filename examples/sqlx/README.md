# sqlx (Rust, PostgreSQL)

A Rust service using [sqlx](https://github.com/launchbadge/sqlx) with its SQL in files: `query_file!` / `query_file_as!` load `queries/*.sql` at compile time, and `migrate!()` applies `migrations/<timestamp>_<name>.sql`. sqlsift checks the same files against the migrations, with no database and no `DATABASE_URL`.

```text
Cargo.toml
sqlsift.toml
src/main.rs                                                   query_file!("queries/open_todos.sql", owner), ...
migrations/20260301090000_create_users.sql
migrations/20260305140000_create_todos.sql
migrations/20260320110000_replace_done_with_completed_at.sql  drops todos.done
queries/*.sql                                                 one statement per file, $1 parameters
broken/*.sql                                                  intentional mistakes
```

`sqlsift.toml` (what `sqlsift init` writes for this layout, minus `broken/`):

```toml
schema_dir = "migrations"
files = ["queries/**/*.sql"]
dialect = "postgresql"
```

The migrations are applied in file name order, so the timestamp prefix gives sqlx's order; the `*.down.sql` halves of reversible migrations are skipped. sqlx's type overrides in column aliases (`id AS "id!"`, `priority AS "priority: Priority"`) are ordinary quoted aliases to sqlsift.

sqlx already checks these queries when it compiles, against a live database or the `.sqlx` cache from `cargo sqlx prepare`. sqlsift checks them in a second, needs neither, and runs where the Rust build doesn't: a pull request that only adds a migration, an editor, a pre-commit hook. The Rust code here is for illustration and isn't built in sqlsift's CI.

## Run it

```console
$ sqlsift check
All 6 file(s) passed validation
```

## What it catches

`broken/` has three mistakes. `overdue_todos.sql` was written before the last migration dropped `todos.done`; with `SQLX_OFFLINE=true` and a stale `.sqlx` cache it still compiles:

```console
$ sqlsift check broken/*.sql
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
    = help: 'done' was dropped by ALTER TABLE in the schema

error[E0003]: Invalid value 'urgent' for enum type 'todo_priority'
  --> broken/set_priority.sql:2:5
    |
  2 | SET priority = 'urgent'
    |     ^^^^^^^^
    = help: Valid values: 'low', 'normal', 'high'

Found 3 error(s), 0 warning(s) in 3 file(s)
```

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
