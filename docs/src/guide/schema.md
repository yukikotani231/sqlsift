# Loading your schema

sqlsift never connects to a database. It builds an in-memory picture of your schema from SQL files: `CREATE TABLE`, `CREATE VIEW`, `CREATE TYPE ... AS ENUM` and `ALTER TABLE` statements, applied in order.

## Schema files and directories

| Option | `sqlsift.toml` key | What it loads |
|--------|--------------------|---------------|
| `--schema <FILE>` / `-s` (repeatable, globs allowed) | `schema = [...]` | The listed files, in the order given |
| `--schema-dir <DIR>` | `schema_dir = "..."` | Every `.sql` file under the directory, recursively, sorted by file name |

Both can be combined. Statements later in the order see the effect of earlier ones, so a migration that renames a column is reflected in the final schema.

## Use it with your stack

sqlsift only needs SQL files for the schema, so it works with whatever produces them.

| Stack | Schema source |
|-------|---------------|
| **Prisma** | `sqlsift check --schema-dir prisma/migrations queries/*.sql` |
| **Rails** (`schema_format = :sql`) | `sqlsift check --schema db/structure.sql queries/*.sql` |
| **sqlx / golang-migrate / Flyway / dbmate** | `sqlsift check --schema-dir migrations queries/*.sql` |
| **`pg_dump --schema-only`** | `sqlsift check --schema schema.sql queries/*.sql` |
| **`mysqldump --no-data`** | `sqlsift check -d mysql --schema schema.sql queries/*.sql` |
| **Hand-written DDL** | `sqlsift check --schema schema/*.sql queries/**/*.sql` |

## Migrations: only the "up" direction

When reading migrations, sqlsift applies only the "up" direction:

- `--schema-dir` skips rollback files: `*.down.sql` (sqlx, golang-migrate) and Flyway undo files `U<version>__*.sql`.
- In any schema file, everything after a dbmate `-- migrate:down` marker (up to the next `-- migrate:up`) is ignored.
- Files passed explicitly with `--schema` are always loaded, even if their name looks like a rollback.

## What sqlsift understands

- `CREATE TABLE` with column types, `NOT NULL`, defaults, primary keys, foreign keys, `UNIQUE` and `CHECK` constraints
- `SERIAL`, `GENERATED ... AS IDENTITY` and `AUTO_INCREMENT` columns (they count as having a default)
- `CREATE VIEW` and `CREATE MATERIALIZED VIEW`, with column names and types inferred from the query
- `CREATE TYPE ... AS ENUM` (and MySQL inline `ENUM(...)` columns)
- `ALTER TABLE`: `ADD` / `DROP` / `RENAME COLUMN`, `ADD CONSTRAINT`, `RENAME TO`

Statements sqlsift doesn't model (functions, triggers, domains, grants, …) are skipped, and the rest of the file is still loaded. Problems while loading the schema (such as an `ALTER TABLE` on a table that doesn't exist) are reported as warnings on stderr by both `check` and `schema`.

## Inspecting the loaded schema

When a query is flagged unexpectedly, check what sqlsift actually understood from your schema. `sqlsift schema` takes the same schema options as `check` (`--schema`, `--schema-dir`, `--config`, `--dialect`) and falls back to `sqlsift.toml`:

```console
$ sqlsift schema --schema-dir migrations
Schema Information:
==================
Dialect: postgresql
Schema files:
  migrations/001_init.sql

Schema: public
  Table: users
    - id integer NOT NULL PRIMARY KEY DEFAULT nextval('users_id_seq'::regclass)
    - name text NOT NULL
    - feeling mood NULL
  View: user_names
    - id integer
    - name text
  Materialized view: user_count
    - n bigint

Enum types:
  mood: 'sad', 'ok', 'happy'
```

Objects are listed per schema in definition order. `sqlsift schema --format json` prints the same information as JSON; see [Output formats](../reference/output-formats.md#schema-json).
