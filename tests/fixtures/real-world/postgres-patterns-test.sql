-- =============================================================================
-- PostgreSQL Patterns Test
-- Schema + 30 common real-world query patterns
-- =============================================================================

-- Schema
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    email TEXT UNIQUE,
    role VARCHAR(20) DEFAULT 'user',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INTEGER REFERENCES users(id),
    amount DECIMAL(10, 2) NOT NULL,
    status VARCHAR(20) DEFAULT 'pending',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE order_items (
    id SERIAL PRIMARY KEY,
    order_id INTEGER REFERENCES orders(id),
    product_name TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    price DECIMAL(10, 2) NOT NULL
);

-- Pattern 1: SELECT * (wildcard)
SELECT * FROM users;

-- Pattern 2: COUNT(*) aggregate
SELECT COUNT(*) FROM orders;

-- Pattern 3: SUM/AVG/MIN/MAX aggregates
SELECT
    SUM(amount) AS total_amount,
    AVG(amount) AS avg_amount,
    MIN(amount) AS min_amount,
    MAX(amount) AS max_amount
FROM orders;

-- Pattern 4: GROUP BY with HAVING
SELECT user_id, COUNT(*) AS order_count, SUM(amount) AS total_spent
FROM orders
GROUP BY user_id
HAVING SUM(amount) > 100;

-- Pattern 5: ORDER BY with LIMIT/OFFSET
SELECT id, name, email
FROM users
ORDER BY created_at DESC
LIMIT 10 OFFSET 20;

-- Pattern 6: DISTINCT
SELECT DISTINCT role FROM users;

-- Pattern 7: UNION / UNION ALL
SELECT id, name, 'active' AS label FROM users WHERE role = 'admin'
UNION ALL
SELECT id, name, 'regular' AS label FROM users WHERE role = 'user';

-- Pattern 8: INSERT ... RETURNING
INSERT INTO users (name, email, role)
VALUES ('Alice', 'alice@example.com', 'admin')
RETURNING id, name, email;

-- Pattern 9: UPDATE ... RETURNING
UPDATE orders
SET status = 'completed'
WHERE id = 1
RETURNING id, status, amount;

-- Pattern 10: DELETE ... RETURNING
DELETE FROM order_items
WHERE order_id = 1
RETURNING id, product_name;

-- Pattern 11: CASE WHEN expression
SELECT
    id,
    name,
    CASE
        WHEN role = 'admin' THEN 'Administrator'
        WHEN role = 'user' THEN 'Regular User'
        ELSE 'Unknown'
    END AS role_label
FROM users;

-- Pattern 12: COALESCE / NULLIF
SELECT
    id,
    COALESCE(email, 'no-email@placeholder.com') AS safe_email,
    NULLIF(role, 'user') AS non_default_role
FROM users;

-- Pattern 13: Type casting (::type and CAST)
SELECT
    id::TEXT AS id_text,
    CAST(amount AS INTEGER) AS amount_int
FROM orders;

-- Pattern 14: EXISTS subquery
SELECT u.id, u.name
FROM users u
WHERE EXISTS (
    SELECT 1 FROM orders o WHERE o.user_id = u.id
);

-- Pattern 15: NOT IN subquery
SELECT id, name
FROM users
WHERE id NOT IN (
    SELECT DISTINCT user_id FROM orders WHERE user_id IS NOT NULL
);

-- Pattern 16: LEFT JOIN with IS NULL (anti-join)
SELECT u.id, u.name
FROM users u
LEFT JOIN orders o ON o.user_id = u.id
WHERE o.id IS NULL;

-- Pattern 17: Schema-qualified name (public.users)
SELECT id, name FROM public.users;

-- Pattern 18: Column alias in ORDER BY
SELECT id, name AS user_name
FROM users
ORDER BY user_name ASC;

-- Pattern 19: Table subquery in FROM (derived table)
SELECT sub.user_id, sub.total_spent
FROM (
    SELECT user_id, SUM(amount) AS total_spent
    FROM orders
    GROUP BY user_id
) AS sub
WHERE sub.total_spent > 500;

-- Pattern 20: INSERT ... ON CONFLICT (upsert)
INSERT INTO users (name, email, role)
VALUES ('Bob', 'bob@example.com', 'user')
ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name;

-- Pattern 21: BETWEEN
SELECT id, amount, created_at
FROM orders
WHERE amount BETWEEN 10.00 AND 500.00;

-- Pattern 22: LIKE / ILIKE
SELECT id, name, email
FROM users
WHERE name LIKE 'A%' OR email ILIKE '%@example.com';

-- Pattern 23: IN list
SELECT id, name
FROM users
WHERE role IN ('admin', 'moderator', 'user');

-- Pattern 24: IS NULL / IS NOT NULL
SELECT id, name, email
FROM users
WHERE email IS NOT NULL AND role IS NOT NULL;

-- Pattern 25: Multiple CTEs
WITH active_users AS (
    SELECT id, name FROM users WHERE role = 'admin'
),
big_orders AS (
    SELECT user_id, SUM(amount) AS total
    FROM orders
    GROUP BY user_id
    HAVING SUM(amount) > 1000
)
SELECT au.id, au.name, bo.total
FROM active_users au
JOIN big_orders bo ON bo.user_id = au.id;

-- Pattern 26: Window function (ROW_NUMBER, RANK)
SELECT
    id,
    user_id,
    amount,
    ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY amount DESC) AS rn,
    RANK() OVER (ORDER BY amount DESC) AS amount_rank
FROM orders;

-- Pattern 27: LATERAL join
SELECT u.id, u.name, lat.recent_order_id, lat.recent_amount
FROM users u,
LATERAL (
    SELECT o.id AS recent_order_id, o.amount AS recent_amount
    FROM orders o
    WHERE o.user_id = u.id
    ORDER BY o.created_at DESC
    LIMIT 1
) lat;

-- Pattern 28: String concatenation (||)
SELECT id, name || ' <' || COALESCE(email, '') || '>' AS display_name
FROM users;

-- Pattern 29: Date arithmetic
SELECT id, created_at, created_at + INTERVAL '30 days' AS expires_at
FROM orders
WHERE created_at > CURRENT_TIMESTAMP - INTERVAL '90 days';

-- Pattern 30: ARRAY operations
SELECT
    id,
    name,
    ARRAY[role] AS roles,
    ARRAY['a', 'b', 'c'] AS tags
FROM users;
