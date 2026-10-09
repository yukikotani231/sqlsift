-- Source tables loaded into the warehouse (the dbt sources)
CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL
);

CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    order_date DATE NOT NULL,
    status TEXT NOT NULL,
    amount_cents INTEGER NOT NULL
);
