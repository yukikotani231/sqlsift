-- +goose Up
CREATE TABLE authors (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    born DATE
);

CREATE TABLE books (
    id BIGSERIAL PRIMARY KEY,
    author_id BIGINT NOT NULL REFERENCES authors (id),
    title TEXT NOT NULL,
    price NUMERIC(8, 2) NOT NULL,
    in_stock BOOLEAN NOT NULL DEFAULT true
);

-- +goose Down
DROP TABLE books;
DROP TABLE authors;
