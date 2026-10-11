CREATE TYPE todo_priority AS ENUM ('low', 'normal', 'high');

CREATE TABLE todos (
    id         BIGSERIAL     PRIMARY KEY,
    owner_id   UUID          NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    title      TEXT          NOT NULL,
    priority   todo_priority NOT NULL DEFAULT 'normal',
    done       BOOLEAN       NOT NULL DEFAULT false,
    due_on     DATE,
    created_at TIMESTAMPTZ   NOT NULL DEFAULT now()
);

CREATE INDEX todos_owner_id_idx ON todos (owner_id);
