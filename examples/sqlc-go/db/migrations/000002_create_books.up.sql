CREATE TYPE book_status AS ENUM ('draft', 'published', 'archived');

CREATE TABLE books (
    id           BIGSERIAL PRIMARY KEY,
    author_id    BIGINT      NOT NULL REFERENCES authors (id) ON DELETE CASCADE,
    title        TEXT        NOT NULL,
    isbn         TEXT        UNIQUE,
    status       book_status NOT NULL DEFAULT 'draft',
    price_cents  INTEGER     NOT NULL CHECK (price_cents >= 0),
    published_at TIMESTAMPTZ,
    tags         TEXT[]      NOT NULL DEFAULT '{}'
);

CREATE INDEX books_author_id_idx ON books (author_id);
