# MySQL

A MySQL schema in Flyway-style versioned migrations (`V1__...sql`), with `?` placeholders, `AUTO_INCREMENT`, inline `ENUM(...)` columns and `INSERT ... ON DUPLICATE KEY UPDATE` in the queries.

```text
sqlsift.toml
migrations/V1__create_catalog.sql
migrations/V2__create_reviews.sql
queries/products.sql
queries/reviews.sql
broken/inventory.sql           intentional mistakes
```

`sqlsift.toml`:

```toml
schema_dir = "migrations"
files = ["queries/**/*.sql"]
dialect = "mysql"
```

## Run it

```console
$ sqlsift check
All 2 file(s) passed validation
```

## What it catches

```console
$ sqlsift check broken/inventory.sql
error[E0003]: Invalid value 'archived' for enum column 'products.status'
  --> broken/inventory.sql:3:5
    |
  3 | SET status = 'archived'
    |     ^^^^^^
    = help: Did you mean 'active'?

error[E0002]: Column 'user_id' not found in table 'reviews'
  --> broken/inventory.sql:7:10
    |
  7 | SELECT r.user_id, r.rating, r.body
    |          ^^^^^^^

warning[E0029]: JOIN condition type mismatch: varchar(32) vs integer
  --> broken/inventory.sql:15:20
    |
 15 | JOIN products p ON p.sku = r.product_id;
    |                    ^^^^^
    = help: MySQL converts the values at run time, so this runs, but rarely does what was meant. Use an explicit CAST if the conversion is intended.

Found 2 error(s), 1 warning(s) in 1 file(s)
```

MySQL compares a string with a number by converting both, so the join runs and quietly matches the wrong rows: sqlsift reports it as a warning. In non-strict SQL mode MySQL also stores an invalid `ENUM` value as an empty string with only a warning, so the first mistake doesn't even fail at run time there.

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
