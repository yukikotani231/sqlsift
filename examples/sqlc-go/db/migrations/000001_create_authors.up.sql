CREATE TABLE authors (
    id         BIGSERIAL PRIMARY KEY,
    name       TEXT        NOT NULL,
    email      TEXT        NOT NULL UNIQUE,
    bio        TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
