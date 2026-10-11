# Plain SQL + migrations (PostgreSQL): a column rename

Plain `.sql` query files next to numbered migrations, as used with sqlx, golang-migrate, Flyway, dbmate or a hand-rolled migration runner. It shows the case sqlsift is built for: **a migration renames a column, and a query that still uses the old name is caught before it ships.**

```text
sqlsift.toml
migrations/0001_create_customers.sql
migrations/0002_create_orders.sql
migrations/0003_rename_customer_name.sql   ALTER TABLE customers RENAME COLUMN name TO full_name
queries/customers.sql                      updated for 0003
queries/orders.sql
broken/top_customers.sql                   still uses customers.name
```

`sqlsift.toml`:

```toml
schema_dir = "migrations"
files = ["queries/**/*.sql"]
dialect = "postgresql"
```

The migrations are applied in file name order, so queries are checked against the schema as it is after the latest one.

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

Migration `0003` renamed `customers.name` to `full_name`, and `queries/` was updated in the same change. `broken/top_customers.sql` is the report query nobody remembered. It still parses, its tests may not touch it, and it would fail only once it runs against the migrated database:

```console
$ sqlsift check broken/top_customers.sql
error[E0002]: Column 'name' not found in table 'customers'
  --> broken/top_customers.sql:4:16
    |
  4 | SELECT c.id, c.name, sum(o.total) AS lifetime_value
    |                ^^^^
    = help: 'name' was renamed to 'full_name' in migrations/0003_rename_customer_name.sql:2

error[E0002]: Column 'name' not found in table 'customers'
  --> broken/top_customers.sql:8:18
    |
  8 | GROUP BY c.id, c.name
    |                  ^^^^
    = help: 'name' was renamed to 'full_name' in migrations/0003_rename_customer_name.sql:2

Found 2 error(s), 0 warning(s) in 1 file(s)
```

In a pull request that only adds the migration, the [GitHub Action](https://sqlsift.dev/docs/integrations/ci.html) reports this as an annotation on the query when it checks every query file, as in the CI guide's setup ([Re-check everything when the schema changes](https://sqlsift.dev/docs/integrations/ci.html#re-check-everything-when-the-schema-changes)).

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
