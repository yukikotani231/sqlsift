CREATE TYPE order_status AS ENUM ('pending', 'paid', 'shipped', 'cancelled');

CREATE TABLE orders (
    id          SERIAL PRIMARY KEY,
    customer_id INTEGER       NOT NULL REFERENCES customers (id),
    status      order_status  NOT NULL DEFAULT 'pending',
    total       NUMERIC(10,2) NOT NULL,
    placed_at   TIMESTAMP     NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE order_items (
    order_id    INTEGER       NOT NULL REFERENCES orders (id) ON DELETE CASCADE,
    sku         VARCHAR(64)   NOT NULL,
    quantity    INTEGER       NOT NULL CHECK (quantity > 0),
    unit_price  NUMERIC(10,2) NOT NULL,
    PRIMARY KEY (order_id, sku)
);
