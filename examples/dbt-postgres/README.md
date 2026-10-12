# dbt on PostgreSQL

A small dbt project (the classic "jaffle shop") with staging models over `{{ source() }}` tables and a mart built from `{{ ref() }}`s. sqlsift masks the Jinja automatically because `dbt_project.yml` is next to `sqlsift.toml`.

```text
dbt_project.yml
sqlsift.toml
warehouse/raw.sql                    DDL of the source tables (raw.customers, raw.orders, raw.payments)
models/staging/sources.yml
models/staging/stg_customers.sql     {{ source('raw', 'customers') }}
models/staging/stg_orders.sql        incremental, {% if is_incremental() %}
models/staging/stg_payments.sql      {% for %} over payment methods
models/marts/customers.sql           {{ ref(...) }}
broken/stg_orders_v2.sql             intentional mistakes
```

`sqlsift.toml`:

```toml
schema = ["warehouse/raw.sql"]
files = ["models/**/*.sql"]
dialect = "postgresql"
```

The DDL of the source tables lets sqlsift check the columns of `{{ source('raw', ...) }}`; write it by hand or take it from `pg_dump --schema-only --schema=raw`. The columns of `{{ ref(...) }}` models are unknown, so they are not reported, until you run `dbt docs generate`: sqlsift then reads them from `target/catalog.json` (alpha).

## Run it

```console
$ sqlsift check
All 4 file(s) passed validation
```

## What it catches

```console
$ sqlsift check broken/stg_orders_v2.sql
error[E0002]: Column 'customer_id' not found in table 'raw.orders'
  --> broken/stg_orders_v2.sql:6:5
    |
  6 |     customer_id,
    |     ^^^^^^^^^^^

error[E0003]: Type mismatch: cannot compare date with integer
  --> broken/stg_orders_v2.sql:10:7
    |
 10 | where order_date >= 20260101
    |       ^^^^^^^^^^
    = help: Types are not implicitly compatible. Consider using explicit CAST.

Found 2 error(s), 0 warning(s) in 1 file(s)
```

Both would otherwise surface only in `dbt run`, against the warehouse.

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).

## License

`models/marts/customers.sql` is adapted from [dbt-labs/jaffle_shop](https://github.com/dbt-labs/jaffle_shop), licensed under the Apache License 2.0 ([`LICENSE-jaffle_shop`](LICENSE-jaffle_shop)). The rest of this example is part of sqlsift (MIT).
