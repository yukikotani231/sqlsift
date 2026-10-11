# Example projects

The repository's [`examples/`](https://github.com/yukikotani231/sqlsift/tree/main/examples) directory has small, realistic projects for common stacks. Each has a schema (usually migrations), a few queries, a hand-written `sqlsift.toml` to copy, and a `broken/` directory with intentional mistakes and the diagnostics they produce. They are checked in sqlsift's CI, so the setup they show keeps working.

| Example | Stack | `sqlsift.toml` |
|---|---|---|
| [sqlc-go](https://github.com/yukikotani231/sqlsift/tree/main/examples/sqlc-go) | sqlc + golang-migrate, PostgreSQL | `schema_dir = "db/migrations"`, `files = ["db/query/**/*.sql"]` |
| [prisma-typedsql](https://github.com/yukikotani231/sqlsift/tree/main/examples/prisma-typedsql) | Prisma Migrate + TypedSQL, `$queryRaw` | `schema_dir = "prisma/migrations"`, `files = ["prisma/sql/*.sql", "src/**/*.ts"]`, `embedded_sql_tags = ["sql", "$queryRaw", "$executeRaw"]` |
| [postgres-js](https://github.com/yukikotani231/sqlsift/tree/main/examples/postgres-js) | postgres.js tagged templates + dbmate | `schema_dir = "db/migrations"`, `files = ["src/**/*.ts"]` |
| [postgres-migrations](https://github.com/yukikotani231/sqlsift/tree/main/examples/postgres-migrations) | Plain SQL + migrations, PostgreSQL | `schema_dir = "migrations"`, `files = ["queries/**/*.sql"]` |
| [mysql](https://github.com/yukikotani231/sqlsift/tree/main/examples/mysql) | Flyway-style migrations, MySQL | `schema_dir = "migrations"`, `files = ["queries/**/*.sql"]`, `dialect = "mysql"` |
| [dbt-postgres](https://github.com/yukikotani231/sqlsift/tree/main/examples/dbt-postgres) | dbt on PostgreSQL | `schema = ["warehouse/raw.sql"]`, `files = ["models/**/*.sql"]` |

## A migration that breaks a query

[postgres-migrations](https://github.com/yukikotani231/sqlsift/tree/main/examples/postgres-migrations) shows the case sqlsift is built for. Migration `0003` renames `customers.name` to `full_name`, and one report query still uses the old name:

```text
error[E0002]: Column 'name' not found in table 'customers'
  --> broken/top_customers.sql:4:16
    |
  4 | SELECT c.id, c.name, sum(o.total) AS lifetime_value
    |                ^^^^
    = help: 'name' was renamed to 'full_name' in migrations/0003_rename_customer_name.sql:2
```

The query still parses and nothing else in the pull request touches it; without sqlsift it fails only once it runs against the migrated database. To get this in pull requests, check every query file in CI (see [Re-check everything when the schema changes](ci.md#re-check-everything-when-the-schema-changes)).
