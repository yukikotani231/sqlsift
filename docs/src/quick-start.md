# Quick start

This page walks through a first check, then saves the options in a configuration file so later runs are just `sqlsift check`.

## The fast way: `sqlsift init`

In an existing project, let sqlsift find the schema, the queries and the dialect:

```console
$ cd my-app
$ sqlsift init
Detected:
  schema    Prisma migrations: prisma/migrations (12 SQL files)
  dialect   postgresql (detected from prisma/schema.prisma: provider = "postgresql")
  queries   Prisma TypedSQL: prisma/sql/**/*.sql (8 files)
            sql`...` / $queryRaw`...` templates in .ts: src/**/*.ts (5 files)

Wrote sqlsift.toml

Running a first check...
  2 error(s), 0 warning(s) in 2 of 31 file(s)
    E0002 column-not-found           2

Next steps:
  - Run `sqlsift check` to see each diagnostic
  ...
```

`init` recognizes sqlc configurations, Prisma, Supabase, Drizzle, Flyway, golang-migrate, sqlx, goose and dbmate migrations, Rails `structure.sql` and other schema dumps, dbt projects, plain `.sql` query files, SQL in TypeScript / JavaScript tagged templates, sqlx queries in Rust and SQL passed to database calls in Python. It writes a commented `sqlsift.toml` (never over an existing one unless you pass `--force`; `--dry-run` only prints it) and runs a first check. Review the file, then use `sqlsift check` from now on. See [`sqlsift init`](reference/cli.md#sqlsift-init) for what is detected and how.

The rest of this page does the same by hand.

## 1. Point sqlsift at your schema and queries

sqlsift needs two things: the SQL that defines your schema, and the query files to check.

```bash
# A single schema file
sqlsift check --schema db/schema.sql queries/*.sql

# Several schema files
sqlsift check -s db/users.sql -s db/orders.sql queries/*.sql

# A directory of migrations, applied in filename order
sqlsift check --schema-dir db/migrations 'queries/**/*.sql'
```

Glob patterns are expanded by sqlsift itself, so quote them when you want `**` to work regardless of your shell.

If every query matches the schema, sqlsift prints a one-line summary and exits with code `0`. Otherwise it prints each problem with its location and a hint, and exits with code `1`.

Not sure where your schema comes from? See [Loading your schema](guide/schema.md) for Prisma, Rails, sqlx, Flyway, dbmate and `pg_dump`.

## 2. Pick the dialect

PostgreSQL is the default. For MySQL or SQLite (or, in alpha, Snowflake, BigQuery, Redshift or Databricks), pass `--dialect`:

```bash
sqlsift check --dialect mysql --schema schema.sql queries/*.sql
```

## 3. Save the options in `sqlsift.toml`

Create `sqlsift.toml` in your project root:

```toml
schema_dir = "db/migrations"
files = ["queries/**/*.sql"]
dialect = "postgresql"
```

Now `sqlsift check` with no arguments checks every query file. sqlsift looks for `sqlsift.toml` in the current directory and its parents, and the VS Code extension reads the same file. See the [configuration reference](reference/config.md) for every key, and the [example projects](integrations/examples.md) for a ready-made `sqlsift.toml` for sqlc, Prisma, postgres.js, sqlx, MySQL and dbt projects.

## 4. Decide what should fail the build

Rules that report queries the database rejects (`correctness`) are errors by default, rules for valid but almost certainly wrong queries (`suspicious`) are warnings, and stricter rules (`restriction`) are off until you enable them. To report a rule without failing the check, make it a warning:

```toml
[rules]
ambiguous-column = "warn"
```

Read [Rules and levels](guide/rules.md) for categories and command-line overrides, and [Suppressing diagnostics](guide/suppression.md) for one-off exceptions in a query file.

## 5. Run it everywhere

- In CI: [CI and GitHub Actions](integrations/ci.md)
- Before each commit: [Pre-commit hooks](integrations/pre-commit.md)
- While you type: [Editors](integrations/editors.md)
