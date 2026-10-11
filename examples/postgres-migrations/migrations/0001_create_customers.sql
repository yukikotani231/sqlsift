CREATE TABLE customers (
    id         SERIAL PRIMARY KEY,
    name       VARCHAR(200) NOT NULL,
    email      VARCHAR(320) NOT NULL UNIQUE,
    country    CHAR(2)      NOT NULL DEFAULT 'US',
    created_at TIMESTAMP    NOT NULL DEFAULT CURRENT_TIMESTAMP
);
