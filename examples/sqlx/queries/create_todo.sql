INSERT INTO todos (owner_id, title, priority, due_on)
VALUES ($1, $2, $3, $4)
RETURNING id
