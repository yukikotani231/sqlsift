-- Applied in file name order
CREATE TABLE authors (
    id         BIGSERIAL PRIMARY KEY,
    name       TEXT        NOT NULL,
    born       DATE
);

CREATE TABLE books (
    id           BIGSERIAL PRIMARY KEY,
    author_id    BIGINT      NOT NULL REFERENCES authors (id),
    title        TEXT        NOT NULL,
    published_on DATE,
    in_stock     BOOLEAN     NOT NULL DEFAULT true
);
