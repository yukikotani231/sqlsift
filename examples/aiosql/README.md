# aiosql (Python, PostgreSQL)

A Python support desk using [aiosql](https://nackjicholson.github.io/aiosql/) to load named queries from `.sql` files, with [yoyo-migrations](https://ollycope.com/software/yoyo/latest/) for the schema. The same setup works for [HugSQL](https://www.hugsql.org) (`-- :name get-user :? :1`) and other libraries that keep named queries in `.sql` files.

```text
sqlsift.toml
requirements.txt
migrations/0001.create-customers.sql
migrations/0002.create-tickets.sql
queries/customers.sql      -- name: get-customer-by-email^, ...
queries/tickets.sql
app/db.py                  aiosql.from_path("queries", "psycopg")
broken/tickets.sql         intentional mistakes, not in sqlsift.toml's files
```

`sqlsift.toml`:

```toml
schema_dir = "migrations"
files = ["queries/**/*.sql"]
dialect = "postgresql"
```

aiosql's `:name` and `:obj.attr` parameters are untyped placeholders, queries don't need a `;` (each `-- name:` comment starts a new one), and diagnostics name the query they are in. The operation suffixes (`^`, `$`, `!`, `<!`, `*!`, `#`) and parameter lists (`-- name: list-open-tickets(customer)`) are not part of the name.

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

`broken/tickets.sql` has three mistakes. aiosql only reads the file and hands the text to the driver, so each fails only when its query runs:

```console
$ sqlsift check broken/tickets.sql
error[E0002]: Column 'subjct' not found in table 'tickets'
  --> broken/tickets.sql:5:12
    |
  5 | select id, subjct, status
    |            ^^^^^^
    = note: in query 'list-tickets-by-assignee'
    = help: Did you mean 'subject'?

error[E0003]: Invalid value 'reopened' for enum type 'ticket_status'
  --> broken/tickets.sql:11:8
    |
 11 |    set status = 'reopened'
    |        ^^^^^^
    = note: in query 'reopen-ticket'
    = help: Valid values: 'open', 'pending', 'solved', 'closed'

error[E0008]: INSERT into 'tickets' is missing required column 'priority'
  --> broken/tickets.sql:15:13
    |
 15 | insert into tickets (customer_id, subject)
    |             ^^^^^^^
    = note: in query 'open-ticket'
    = help: 'priority' is NOT NULL without a default. Provide a value, or add a DEFAULT to the schema

Found 3 error(s), 0 warning(s) in 1 file(s)
```

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
