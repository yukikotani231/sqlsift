# database/sql, sqlx and pgx (Go, PostgreSQL)

A Go service that keeps its SQL in string literals: [`database/sql`](https://pkg.go.dev/database/sql) (`db.QueryRowContext(ctx, "...", args...)`), [sqlx](https://jmoiron.github.io/sqlx/) (`db.SelectContext(ctx, &dest, "...")`, `db.NamedExec("...", arg)`, `sqlx.In`) and [pgx](https://github.com/jackc/pgx) (`pool.Exec(ctx, "...", pgx.NamedArgs{...})`), with [goose](https://github.com/pressly/goose) migrations for the schema. The same setup works for lib/pq, go-sql-driver/mysql and the SQLite drivers.

```text
sqlsift.toml
go.mod
migrations/00001_create_books.sql   -- +goose Up / -- +goose Down
migrations/00002_add_isbn.sql
internal/store/books.go            db.QueryRowContext(ctx, `SELECT ... WHERE b.id = $1`, id)
internal/store/stock.go            pool.Exec(ctx, "UPDATE ... = @in_stock ...", pgx.NamedArgs{...})
broken/catalog.go                  intentional mistakes, not in sqlsift.toml's files
```

`sqlsift.toml`:

```toml
schema_dir = "migrations"
files = ["internal/**/*.go"]
dialect = "postgresql"
```

Only string literals passed where the query goes in `Query`, `QueryRow`, `Exec`, `Prepare` (and their `Context` variants), sqlx's `Get`, `Select`, `NamedExec`, `NamedQuery`, `sqlx.In` and a few others are checked (`embedded_sql_go_functions` in `sqlsift.toml` changes the list), so other strings never produce diagnostics. Raw strings (`` `...` ``) and literals joined with `+` are one query. `$1`, `?`, `:name` and `@name` are untyped parameters, and `IN (?)` is a list. `fmt.Sprintf`, constants and SQL joined with variables are only known at run time and are skipped. The goose `Down` sections are not part of the schema.

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

`broken/catalog.go` has three mistakes. The driver sends each query only when its function runs, so none of them shows up before that:

```console
$ sqlsift check broken/catalog.go
error[E0002]: Column 'titel' not found in table 'books'
  --> broken/catalog.go:11:42
    |
 11 |     return db.QueryContext(ctx, "SELECT id, titel FROM books WHERE title ILIKE $1", "%"+term+"%")
    |                                          ^^^^^
    = help: Did you mean 'title'?

error[E0003]: Type mismatch: column 'in_stock' expects boolean, but got text
  --> broken/catalog.go:15:50
    |
 15 |     _, err := db.ExecContext(ctx, "UPDATE books SET in_stock = 'yes please' WHERE id = $1", id)
    |                                                  ^^^^^^^^
    = help: Value type is not compatible with the column type. Consider using explicit CAST.

error[E0008]: INSERT into 'authors' is missing required column 'name'
  --> broken/catalog.go:20:45
    |
 20 |     _, err := db.ExecContext(ctx, "INSERT INTO authors (born) VALUES ($1)", "1970-01-01")
    |                                             ^^^^^^^
    = help: 'name' is NOT NULL without a default. Provide a value, or add a DEFAULT to the schema


Found 3 error(s), 0 warning(s) in 1 file(s)
```
