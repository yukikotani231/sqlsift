-- migrate:up
CREATE TABLE projects (
    id          BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id  UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    settings    JSONB       NOT NULL DEFAULT '{}',
    archived_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, name)
);

CREATE TABLE events (
    id          BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    project_id  BIGINT      NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    kind        TEXT        NOT NULL,
    payload     JSONB,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- migrate:down
DROP TABLE events;
DROP TABLE projects;
