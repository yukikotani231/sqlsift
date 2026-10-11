# sqlc (Go, PostgreSQL)

A [sqlc](https://sqlc.dev) project with [golang-migrate](https://github.com/golang-migrate/migrate) migrations. sqlsift reads the same paths as `sqlc.yaml`: the schema from `db/migrations` (the `*.down.sql` rollbacks are skipped) and the queries from `db/query`.

```text
sqlc.yaml
sqlsift.toml
db/migrations/000001_create_authors.{up,down}.sql
db/migrations/000002_create_books.{up,down}.sql
db/query/authors.sql      -- name: GetAuthor :one, ...
db/query/books.sql
broken/books.sql          intentional mistakes, not in sqlc.yaml
```

`sqlsift.toml`:

```toml
schema_dir = "db/migrations"
files = ["db/query/**/*.sql"]
dialect = "postgresql"
```

sqlc's parameters (`$1`, `@name`, `sqlc.arg(...)`, `sqlc.narg(...)`, `sqlc.slice(...)`) are untyped placeholders, and diagnostics name the `-- name:` query they are in.

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

`broken/books.sql` has three mistakes. The enum value and the missing `NOT NULL` column get past code generation and fail only when the query runs:

```console
$ sqlsift check broken/books.sql
error[E0002]: Column 'titel' not found in table 'books'
  --> broken/books.sql:5:12
    |
  5 | SELECT id, titel, published_at
    |            ^^^^^
    = note: in query 'ListPublishedBooks'
    = help: Did you mean 'title'?

error[E0003]: Invalid value 'retired' for enum type 'book_status'
  --> broken/books.sql:11:5
    |
 11 | SET status = 'retired'
    |     ^^^^^^
    = note: in query 'ArchiveBook'
    = help: Valid values: 'draft', 'published', 'archived'

error[E0008]: INSERT into 'books' is missing required column 'price_cents'
  --> broken/books.sql:15:13
    |
 15 | INSERT INTO books (author_id, title)
    |             ^^^^^
    = note: in query 'CreateBook'
    = help: 'price_cents' is NOT NULL without a default. Provide a value, or add a DEFAULT to the schema

Found 3 error(s), 0 warning(s) in 1 file(s)
```

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
