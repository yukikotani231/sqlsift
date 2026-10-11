# psycopg and SQLAlchemy (Python, PostgreSQL)

A Python app that keeps its SQL in string literals: [psycopg](https://www.psycopg.org/psycopg3/) cursors (`cur.execute("...", params)`) and [SQLAlchemy](https://www.sqlalchemy.org) Core (`conn.execute(text("..."))`), with plain SQL migrations for the schema. The same setup works for sqlite3, MySQLdb / PyMySQL, asyncpg (`await conn.fetch("...")`) and Django's `raw()`.

```text
sqlsift.toml
requirements.txt
migrations/0001_create_books.sql
migrations/0002_add_isbn.sql
app/books.py               cur.execute("""SELECT ... WHERE b.id = %s""", (book_id,))
app/reports.py             conn.execute(text("SELECT ... >= :since"), ...)
broken/catalog.py          intentional mistakes, not in sqlsift.toml's files
```

`sqlsift.toml`:

```toml
schema_dir = "migrations"
files = ["app/**/*.py"]
dialect = "postgresql"
```

Only string literals passed as the first argument of `execute`, `executemany`, `fetch`, `fetchrow`, `fetchval`, `text`, `raw` and a few others are checked (`embedded_sql_functions` in `sqlsift.toml` changes the list), so other strings never produce diagnostics. Triple-quoted strings and literals written side by side (`"SELECT ... " "FROM ..."`) are one query. `%s`, `%(name)s` and `:name` are untyped parameters, and `IN %(ids)s` is a list. f-strings and SQL built with `+` or `%` are only known at run time and are skipped.

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

`broken/catalog.py` has three mistakes. The driver sends each query only when its function runs, so none of them shows up before that:

```console
$ sqlsift check broken/catalog.py
error[E0002]: Column 'titel' not found in table 'books'
  --> broken/catalog.py:5:29
    |
  5 |     cur.execute("SELECT id, titel FROM books WHERE title ILIKE %s", (f"%{term}%",))
    |                             ^^^^^
    = help: Did you mean 'title'?

error[E0003]: Type mismatch: column 'in_stock' expects boolean, but got text
  --> broken/catalog.py:9:35
    |
  9 |     cur.execute("UPDATE books SET in_stock = 'yes please' WHERE id = %s", (book_id,))
    |                                   ^^^^^^^^

error[E0008]: INSERT into 'authors' is missing required column 'name'
  --> broken/catalog.py:13:30
    |
 13 |     cur.execute("INSERT INTO authors (born) VALUES (%s)", ("1970-01-01",))
    |                              ^^^^^^^

Found 3 error(s), 0 warning(s) in 1 file(s)
```
