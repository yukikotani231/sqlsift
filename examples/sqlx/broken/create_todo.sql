INSERT INTO todos (title, priority)
VALUES ($1, $2)
RETURNING id
