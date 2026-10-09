# Quick start

This page walks through a first check, then saves the options in a configuration file so later runs are just `sqlsift check`.

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

PostgreSQL is the default. For MySQL or SQLite, pass `--dialect`:

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

Now `sqlsift check` with no arguments checks every query file. sqlsift looks for `sqlsift.toml` in the current directory and its parents, and the VS Code extension reads the same file. See the [configuration reference](reference/config.md) for every key.

## 4. Decide what should fail the build

Every rule is an error by default. To report a rule without failing the check, make it a warning:

```toml
[rules]
ambiguous-column = "warn"
```

Read [Rules and levels](guide/rules.md) for categories and command-line overrides, and [Suppressing diagnostics](guide/suppression.md) for one-off exceptions in a query file.

## 5. Run it everywhere

- In CI: [CI and GitHub Actions](integrations/ci.md)
- Before each commit: [Pre-commit hooks](integrations/pre-commit.md)
- While you type: [Editors](integrations/editors.md)
