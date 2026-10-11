SELECT id, email, display_name, created_at
FROM users
WHERE email = $1
