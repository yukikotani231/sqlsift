-- =============================================================================
-- PostgreSQL Advanced Patterns Test
-- Additional real-world patterns NOT covered by postgres-patterns-test.sql
-- Focus: patterns likely to fail in the resolver
-- =============================================================================

-- Schema (same as postgres-patterns-test.sql)
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

-- Additional tables for advanced patterns
CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT,
    price DECIMAL(10, 2) NOT NULL,
    metadata JSONB,
    tags TEXT[],
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE audit_log (
    id SERIAL PRIMARY KEY,
    table_name TEXT NOT NULL,
    record_id INTEGER NOT NULL,
    action VARCHAR(10) NOT NULL,
    old_data JSONB,
    new_data JSONB,
    performed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    performed_by INTEGER REFERENCES users(id)
);

-- =============================================================================
-- Pattern 31: EXTRACT (date part extraction)
-- ORM-generated: Rails/Django date queries
-- Tests Expr::Extract handling in resolver
-- =============================================================================
SELECT
    id,
    EXTRACT(YEAR FROM created_at) AS order_year,
    EXTRACT(MONTH FROM created_at) AS order_month,
    amount
FROM orders
WHERE EXTRACT(YEAR FROM created_at) = 2024;

-- =============================================================================
-- Pattern 32: Recursive CTE
-- Common for tree/graph traversal (org charts, categories, threaded comments)
-- Tests recursive CTE handling
-- =============================================================================
WITH RECURSIVE subordinates AS (
    SELECT id, name, role
    FROM users
    WHERE role = 'admin'
    UNION ALL
    SELECT u.id, u.name, u.role
    FROM users u
    INNER JOIN subordinates s ON s.id = u.id
)
SELECT id, name, role FROM subordinates;

-- =============================================================================
-- Pattern 33: JSONB operators (->>, ->, #>, @>, ?)
-- Common in Rails/Django with JSONB columns
-- Tests Expr::JsonAccess / BinaryOp with JSON operators
-- =============================================================================
SELECT
    id,
    name,
    metadata->>'color' AS color,
    metadata->'dimensions' AS dimensions
FROM products
WHERE metadata @> '{"active": true}'::jsonb;

-- =============================================================================
-- Pattern 34: ANY/ALL with subquery
-- ORM-generated: SQLAlchemy any_() / Django __in with subquery
-- Tests Expr::AnyOp/AllOp handling
-- =============================================================================
SELECT id, name
FROM users
WHERE id = ANY(SELECT user_id FROM orders WHERE amount > 100);

-- =============================================================================
-- Pattern 35: FILTER clause on aggregate
-- Reporting/analytics: conditional aggregates
-- Tests aggregate FILTER handling
-- =============================================================================
SELECT
    user_id,
    COUNT(*) AS total_orders,
    COUNT(*) FILTER (WHERE status = 'completed') AS completed_orders,
    SUM(amount) FILTER (WHERE status = 'completed') AS completed_amount
FROM orders
GROUP BY user_id;

-- =============================================================================
-- Pattern 36: IS TRUE / IS FALSE / IS UNKNOWN
-- Application logic patterns
-- Tests Expr::IsTrue/IsFalse handling
-- =============================================================================
SELECT id, name
FROM users
WHERE (role = 'admin') IS TRUE;

-- =============================================================================
-- Pattern 37: UPDATE ... FROM (PostgreSQL extension)
-- Common data migration / batch update pattern
-- Tests UPDATE with FROM clause handling
-- =============================================================================
UPDATE orders
SET status = 'vip'
FROM users
WHERE users.id = orders.user_id
  AND users.role = 'admin';

-- =============================================================================
-- Pattern 38: DELETE ... USING (PostgreSQL extension)
-- Common data cleanup pattern
-- Tests DELETE with USING clause handling
-- =============================================================================
DELETE FROM order_items
USING orders
WHERE order_items.order_id = orders.id
  AND orders.status = 'cancelled';

-- =============================================================================
-- Pattern 39: Scalar subquery in SELECT list (correlated)
-- Reporting pattern: inline aggregated values
-- Tests correlated subquery column resolution in SELECT
-- =============================================================================
SELECT
    u.id,
    u.name,
    (SELECT COUNT(*) FROM orders o WHERE o.user_id = u.id) AS order_count,
    (SELECT COALESCE(SUM(o.amount), 0) FROM orders o WHERE o.user_id = u.id) AS total_spent
FROM users u;

-- =============================================================================
-- Pattern 40: Multiple JOINs with mixed types
-- ORM eager loading (Rails includes/joins, Django select_related)
-- Tests multi-table join scope
-- =============================================================================
SELECT
    u.name AS customer_name,
    o.id AS order_id,
    o.amount,
    oi.product_name,
    oi.quantity,
    oi.price
FROM users u
INNER JOIN orders o ON o.user_id = u.id
LEFT JOIN order_items oi ON oi.order_id = o.id
WHERE o.status = 'completed'
ORDER BY o.created_at DESC;

-- =============================================================================
-- Pattern 41: GROUPING SETS / CUBE / ROLLUP
-- Analytics/reporting: multi-dimensional aggregation
-- Tests GROUP BY extensions
-- =============================================================================
SELECT
    user_id,
    status,
    COUNT(*) AS cnt,
    SUM(amount) AS total
FROM orders
GROUP BY GROUPING SETS (
    (user_id, status),
    (user_id),
    (status),
    ()
);

-- =============================================================================
-- Pattern 42: String functions (SUBSTRING, POSITION, TRIM, UPPER, LOWER)
-- Application layer string manipulation
-- Tests Expr::Substring, Expr::Trim, Expr::Position handling
-- =============================================================================
SELECT
    id,
    UPPER(name) AS upper_name,
    LOWER(email) AS lower_email,
    SUBSTRING(name FROM 1 FOR 3) AS name_prefix,
    TRIM(BOTH ' ' FROM name) AS trimmed_name,
    POSITION('@' IN email) AS at_position
FROM users;

-- =============================================================================
-- Pattern 43: INSERT ... SELECT (data migration / ETL)
-- Common in migration scripts and batch operations
-- Tests INSERT with SELECT source
-- =============================================================================
INSERT INTO audit_log (table_name, record_id, action, performed_by)
SELECT 'orders', o.id, 'archive', o.user_id
FROM orders o
WHERE o.status = 'completed'
  AND o.created_at < CURRENT_TIMESTAMP - INTERVAL '1 year';

-- =============================================================================
-- Pattern 44: Window functions with ROWS/RANGE frame
-- Analytics: running totals, moving averages
-- Tests window frame clause handling
-- =============================================================================
SELECT
    id,
    user_id,
    amount,
    SUM(amount) OVER (
        PARTITION BY user_id
        ORDER BY created_at
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    ) AS running_total,
    AVG(amount) OVER (
        PARTITION BY user_id
        ORDER BY created_at
        ROWS BETWEEN 2 PRECEDING AND CURRENT ROW
    ) AS moving_avg
FROM orders;

-- =============================================================================
-- Pattern 45: COALESCE in JOIN condition + complex WHERE
-- ORM-generated: optional relationship filtering
-- Tests expression complexity in ON clauses
-- =============================================================================
SELECT u.id, u.name, COALESCE(o.amount, 0) AS last_amount
FROM users u
LEFT JOIN orders o ON o.user_id = u.id AND o.status = 'completed'
WHERE u.created_at >= CURRENT_TIMESTAMP - INTERVAL '30 days'
   OR u.role IN ('admin', 'moderator');

-- =============================================================================
-- Pattern 46: CTE used in INSERT (write CTE)
-- Data migration pattern
-- Tests CTE scope in INSERT statements
-- =============================================================================
WITH recent_orders AS (
    SELECT id, user_id, amount, status
    FROM orders
    WHERE created_at > CURRENT_TIMESTAMP - INTERVAL '7 days'
)
SELECT id, user_id, amount, status
FROM recent_orders
WHERE amount > 100;

-- =============================================================================
-- Pattern 47: DISTINCT ON (PostgreSQL-specific)
-- Common for "latest per group" queries
-- Tests DISTINCT ON handling
-- =============================================================================
SELECT DISTINCT ON (user_id)
    id,
    user_id,
    amount,
    status,
    created_at
FROM orders
ORDER BY user_id, created_at DESC;

-- =============================================================================
-- Pattern 48: AT TIME ZONE
-- Application timezone handling
-- Tests Expr::AtTimeZone handling
-- =============================================================================
SELECT
    id,
    created_at,
    created_at AT TIME ZONE 'UTC' AT TIME ZONE 'America/New_York' AS eastern_time
FROM orders
WHERE created_at AT TIME ZONE 'UTC' > CURRENT_DATE;

-- =============================================================================
-- Pattern 49: ARRAY_AGG / STRING_AGG (aggregate functions returning non-scalar)
-- Reporting: denormalized output
-- Tests aggregate functions with ORDER BY inside
-- =============================================================================
SELECT
    u.id,
    u.name,
    ARRAY_AGG(o.amount ORDER BY o.created_at DESC) AS order_amounts,
    STRING_AGG(o.status, ', ' ORDER BY o.created_at) AS status_history
FROM users u
JOIN orders o ON o.user_id = u.id
GROUP BY u.id, u.name;

-- =============================================================================
-- Pattern 50: Chained type cast with complex expression
-- Data transformation pattern
-- Tests Cast expressions nested in other expressions
-- =============================================================================
SELECT
    id,
    (amount * 100)::INTEGER AS amount_cents,
    CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS epoch_seconds,
    created_at::DATE AS order_date
FROM orders;

-- =============================================================================
-- Pattern 51: NATURAL JOIN
-- Less common but used in some ORMs
-- Tests NATURAL JOIN handling (no ON clause)
-- =============================================================================
SELECT users.id, users.name, orders.amount
FROM users
NATURAL JOIN orders;

-- =============================================================================
-- Pattern 52: CROSS JOIN
-- Cartesian product (reporting matrices)
-- Tests CROSS JOIN handling
-- =============================================================================
SELECT u.name, s.status
FROM users u
CROSS JOIN (SELECT DISTINCT status FROM orders) AS s;

-- =============================================================================
-- Pattern 53: Multiple subqueries in WHERE clause
-- Complex filtering (application search)
-- Tests multiple independent subqueries
-- =============================================================================
SELECT id, name, email
FROM users
WHERE id IN (SELECT user_id FROM orders WHERE amount > 100)
  AND id NOT IN (SELECT user_id FROM orders WHERE status = 'cancelled' AND user_id IS NOT NULL)
  AND EXISTS (SELECT 1 FROM order_items oi JOIN orders o ON o.id = oi.order_id WHERE o.user_id = users.id AND oi.quantity > 5);

-- =============================================================================
-- Pattern 54: Qualified wildcard (table.*)
-- ORM eager loading: select all columns from one table in a JOIN
-- Tests QualifiedWildcard with aliased tables
-- =============================================================================
SELECT u.*, o.amount, o.status
FROM users u
JOIN orders o ON o.user_id = u.id;

-- =============================================================================
-- Pattern 55: GENERATE_SERIES and set-returning functions in FROM
-- Data generation for reports
-- Tests function calls in FROM clause (TableFactor::Function)
-- =============================================================================
SELECT d::DATE AS report_date
FROM generate_series(
    CURRENT_DATE - INTERVAL '30 days',
    CURRENT_DATE,
    INTERVAL '1 day'
) AS d;

-- =============================================================================
-- Pattern 56: VALUES as standalone query (not in INSERT)
-- Used for ad-hoc data / inline tables
-- Tests VALUES outside of INSERT context
-- =============================================================================
SELECT v.status, v.label
FROM (VALUES ('pending', 'Pending'), ('completed', 'Done'), ('cancelled', 'Cancelled')) AS v(status, label);

-- =============================================================================
-- Pattern 57: EXCEPT / INTERSECT
-- Set operations beyond UNION
-- Tests SetOperation variants
-- =============================================================================
SELECT id FROM users WHERE role = 'admin'
EXCEPT
SELECT user_id FROM orders WHERE status = 'cancelled';

-- =============================================================================
-- Pattern 58: Nested CASE with aggregate
-- Complex reporting logic
-- Tests deeply nested CASE expressions inside aggregates
-- =============================================================================
SELECT
    user_id,
    SUM(CASE
        WHEN status = 'completed' THEN amount
        WHEN status = 'pending' THEN amount * 0.5
        ELSE 0
    END) AS weighted_total,
    COUNT(CASE WHEN status = 'completed' THEN 1 END) AS completed_count
FROM orders
GROUP BY user_id;

-- =============================================================================
-- Pattern 59: Common ORM pagination (keyset pagination)
-- Application CRUD: cursor-based pagination
-- Tests compound WHERE with tuple comparison
-- =============================================================================
SELECT id, name, created_at
FROM users
WHERE (created_at, id) < ('2024-01-01'::TIMESTAMP, 100)
ORDER BY created_at DESC, id DESC
LIMIT 20;

-- =============================================================================
-- Pattern 60: FOR UPDATE / FOR SHARE (row locking)
-- Transactional application patterns
-- Tests SELECT ... FOR UPDATE parsing
-- =============================================================================
SELECT id, name, email, role
FROM users
WHERE id = 1
FOR UPDATE;
