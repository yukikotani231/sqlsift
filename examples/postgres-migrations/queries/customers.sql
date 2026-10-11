-- Customer lookup and signup, updated for migration 0003.
SELECT id, full_name, coalesce(display_name, full_name) AS shown_name, email
FROM customers
WHERE email = $1;

INSERT INTO customers (full_name, email, country)
VALUES ($1, $2, $3)
RETURNING id;

UPDATE customers
SET display_name = $2
WHERE id = $1;
