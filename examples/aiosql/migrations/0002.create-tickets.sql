-- depends: 0001.create-customers
CREATE TYPE ticket_status AS ENUM ('open', 'pending', 'solved', 'closed');

CREATE TABLE tickets (
    id          BIGSERIAL PRIMARY KEY,
    customer_id BIGINT        NOT NULL REFERENCES customers (id) ON DELETE CASCADE,
    subject     TEXT          NOT NULL,
    status      ticket_status NOT NULL DEFAULT 'open',
    priority    SMALLINT      NOT NULL CHECK (priority BETWEEN 1 AND 4),
    assignee    TEXT,
    opened_at   TIMESTAMPTZ   NOT NULL DEFAULT now(),
    solved_at   TIMESTAMPTZ
);

CREATE TABLE ticket_comments (
    id         BIGSERIAL PRIMARY KEY,
    ticket_id  BIGINT      NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    author     TEXT        NOT NULL,
    body       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX tickets_customer_id_idx ON tickets (customer_id);
