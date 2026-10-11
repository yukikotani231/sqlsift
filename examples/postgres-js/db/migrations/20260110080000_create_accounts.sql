-- migrate:up
CREATE TABLE accounts (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email      TEXT        NOT NULL UNIQUE,
    plan       TEXT        NOT NULL DEFAULT 'free' CHECK (plan IN ('free', 'pro', 'team')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- migrate:down
DROP TABLE accounts;
