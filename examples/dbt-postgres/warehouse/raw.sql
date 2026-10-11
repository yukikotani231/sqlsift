-- The source tables the models read, as they are in the warehouse (for example
-- from `pg_dump --schema-only --schema=raw`). dbt doesn't create them, so
-- sqlsift needs their DDL to check the columns of {{ source(...) }}.
CREATE SCHEMA raw;

CREATE TABLE raw.customers (
    id         INTEGER PRIMARY KEY,
    first_name TEXT NOT NULL,
    last_name  TEXT NOT NULL,
    email      TEXT,
    created_at TIMESTAMP NOT NULL
);

CREATE TABLE raw.orders (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL,
    order_date DATE    NOT NULL,
    status     TEXT    NOT NULL
);

CREATE TABLE raw.payments (
    id             INTEGER PRIMARY KEY,
    order_id       INTEGER NOT NULL,
    payment_method TEXT    NOT NULL,
    amount         INTEGER NOT NULL  -- in cents
);
