INSERT INTO users (email, display_name)
VALUES ($1, $2)
RETURNING id, email, display_name, created_at
