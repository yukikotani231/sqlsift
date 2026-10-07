//! PostgreSQL query analysis coverage tests.
//!
//! These tests exercise the default (PostgreSQL) dialect against a realistic
//! schema. They come in two flavours:
//!
//! * valid queries that must produce zero diagnostics (false-positive hunting)
//! * invalid queries that must produce a specific diagnostic (true positives)

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const SCHEMA: &str = r#"
CREATE TYPE order_status AS ENUM ('pending', 'paid', 'shipped', 'cancelled');
CREATE TYPE user_role AS ENUM ('admin', 'member', 'guest');

CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    uuid UUID NOT NULL UNIQUE,
    email TEXT NOT NULL UNIQUE,
    name VARCHAR(100) NOT NULL,
    role user_role NOT NULL DEFAULT 'member',
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    manager_id BIGINT REFERENCES users(id),
    tags TEXT[],
    settings JSONB,
    balance NUMERIC(12, 2) NOT NULL DEFAULT 0,
    birth_date DATE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMP
);

CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    sku TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    price NUMERIC(10, 2) NOT NULL CHECK (price >= 0),
    stock INTEGER NOT NULL DEFAULT 0,
    attributes JSONB,
    ratings INTEGER[],
    discontinued BOOLEAN DEFAULT FALSE
);

CREATE TABLE orders (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    status order_status NOT NULL DEFAULT 'pending',
    total NUMERIC(12, 2) NOT NULL,
    note TEXT,
    placed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    shipped_at TIMESTAMPTZ
);

CREATE TABLE order_items (
    order_id BIGINT NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id INTEGER NOT NULL REFERENCES products(id),
    quantity SMALLINT NOT NULL CHECK (quantity > 0),
    unit_price NUMERIC(10, 2) NOT NULL,
    PRIMARY KEY (order_id, product_id)
);

CREATE TABLE categories (
    id SERIAL PRIMARY KEY,
    parent_id INTEGER REFERENCES categories(id),
    name TEXT NOT NULL
);

CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT REFERENCES users(id),
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    duration INTERVAL,
    ip INET
);

CREATE TABLE "AuditLog" (
    "Id" SERIAL PRIMARY KEY,
    "UserId" BIGINT,
    "Action" TEXT NOT NULL,
    "CreatedAt" TIMESTAMPTZ
);

CREATE SCHEMA billing;

CREATE TABLE billing.invoices (
    id SERIAL PRIMARY KEY,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    amount NUMERIC(12, 2) NOT NULL,
    currency CHAR(3) NOT NULL DEFAULT 'USD',
    issued_on DATE NOT NULL,
    paid BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE VIEW active_users AS
    SELECT id, email, name, role FROM users WHERE is_active;

CREATE VIEW order_totals AS
    SELECT o.user_id, count(*) AS order_count, sum(o.total) AS revenue
    FROM orders o
    GROUP BY o.user_id;
"#;

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).expect("schema must parse");
    let (catalog, diags) = builder.build();
    assert!(diags.is_empty(), "schema diagnostics: {diags:#?}");
    catalog
}

fn analyze(catalog: &Catalog, sql: &str) -> Vec<Diagnostic> {
    Analyzer::new(catalog).analyze(sql)
}

fn fmt_diags(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            let loc = d
                .span
                .map(|s| format!("{}:{}", s.line, s.column))
                .unwrap_or_else(|| "-".into());
            format!(
                "    [{} @ {}] {} (help: {:?})",
                d.code(),
                loc,
                d.message,
                d.help
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert that every query in `cases` produces zero diagnostics.
/// All failures are collected so that one run reports every false positive.
fn assert_all_valid(group: &str, cases: &[&str]) {
    let catalog = catalog();
    let mut failures = Vec::new();
    for sql in cases {
        let diags = analyze(&catalog, sql);
        if !diags.is_empty() {
            failures.push(format!("  SQL: {sql}\n{}", fmt_diags(&diags)));
        }
    }
    assert!(
        failures.is_empty(),
        "[{group}] {} of {} valid queries produced diagnostics:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// Assert that every query produces at least one diagnostic of the given kind.
fn assert_all_error(group: &str, cases: &[(&str, DiagnosticKind)]) {
    let catalog = catalog();
    let mut failures = Vec::new();
    for (sql, kind) in cases {
        let diags = analyze(&catalog, sql);
        if !diags.iter().any(|d| d.kind == *kind) {
            failures.push(format!(
                "  SQL: {sql}\n  expected {}, got:\n{}",
                kind.code(),
                fmt_diags(&diags)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "[{group}] {} of {} invalid queries missed the expected diagnostic:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// Assert that the query produces exactly one diagnostic of `kind`, located at
/// `line`:`column`, whose message contains `msg_part` and (optionally) whose
/// help text contains `help_part`.
fn assert_single(
    sql: &str,
    kind: DiagnosticKind,
    line: usize,
    column: usize,
    msg_part: &str,
    help_part: Option<&str>,
) {
    let catalog = catalog();
    let diags = analyze(&catalog, sql);
    let ctx = format!("SQL: {sql}\n{}", fmt_diags(&diags));
    assert_eq!(diags.len(), 1, "expected exactly one diagnostic\n{ctx}");
    let d = &diags[0];
    assert_eq!(d.kind, kind, "wrong kind\n{ctx}");
    let span = d.span.unwrap_or_else(|| panic!("missing span\n{ctx}"));
    assert_eq!(
        (span.line, span.column),
        (line, column),
        "wrong span\n{ctx}"
    );
    assert!(d.message.contains(msg_part), "message mismatch\n{ctx}");
    if let Some(h) = help_part {
        assert!(
            d.help.as_deref().is_some_and(|x| x.contains(h)),
            "help mismatch (wanted {h:?})\n{ctx}"
        );
    }
}

// ===========================================================================
// Schema sanity
// ===========================================================================

#[test]
fn schema_builds_cleanly() {
    let catalog = catalog();
    let diags = analyze(&catalog, "SELECT 1");
    assert!(diags.is_empty());
}

// ===========================================================================
// VALID: SELECT clauses
// ===========================================================================

#[test]
fn valid_select_clauses() {
    assert_all_valid(
        "select clauses",
        &[
            "SELECT * FROM users",
            "SELECT u.* FROM users u",
            "SELECT users.id, users.email FROM users",
            "SELECT DISTINCT role FROM users",
            "SELECT DISTINCT ON (user_id) user_id, id, placed_at FROM orders ORDER BY user_id, placed_at DESC",
            "SELECT role, count(*) FROM users GROUP BY 1",
            "SELECT role, count(*) FROM users GROUP BY role HAVING count(*) > 5",
            "SELECT date_trunc('month', placed_at), sum(total) FROM orders GROUP BY date_trunc('month', placed_at)",
            "SELECT user_id, sum(total) AS spent FROM orders GROUP BY user_id HAVING sum(total) > 100 ORDER BY spent DESC",
            "SELECT name AS n FROM users ORDER BY n",
            "SELECT name, email FROM users ORDER BY 2 DESC, 1",
            "SELECT * FROM orders ORDER BY shipped_at DESC NULLS LAST",
            "SELECT * FROM orders ORDER BY shipped_at ASC NULLS FIRST, id",
            "SELECT * FROM users LIMIT 10 OFFSET 20",
            "SELECT * FROM users LIMIT ALL",
            "SELECT * FROM users OFFSET 5 ROWS FETCH FIRST 10 ROWS ONLY",
            "SELECT * FROM users FETCH NEXT 1 ROW ONLY",
            "SELECT * FROM orders WHERE id = 1 FOR UPDATE",
            "SELECT * FROM orders WHERE id = 1 LIMIT 1 FOR UPDATE SKIP LOCKED",
            "SELECT * FROM orders FOR SHARE NOWAIT",
            "SELECT * FROM orders o FOR UPDATE OF o",
            "SELECT 1",
            "SELECT 1 + 2 AS three, 'x' AS letter",
            "SELECT now()",
            "SELECT count(*) FROM users",
            "SELECT count(DISTINCT role) FROM users",
            "SELECT id FROM users WHERE is_active",
            "SELECT id FROM users WHERE NOT is_active",
            "SELECT id FROM users WHERE is_active AND manager_id IS NULL",
            "SELECT id FROM users WHERE is_active IS TRUE",
            "SELECT id FROM users WHERE is_active IS NOT FALSE",
            "SELECT id FROM users WHERE manager_id IS NOT NULL",
            "SELECT id FROM users WHERE id IN (1, 2, 3)",
            "SELECT id FROM users WHERE id NOT IN (1, 2)",
            "SELECT id FROM users WHERE id BETWEEN 1 AND 10",
            "SELECT id FROM users WHERE id NOT BETWEEN 1 AND 10",
            "SELECT id FROM users WHERE manager_id IS DISTINCT FROM 5",
            "SELECT id FROM users WHERE manager_id IS NOT DISTINCT FROM id",
            "SELECT id FROM users WHERE (id, manager_id) = (1, 2)",
            "SELECT id, name FROM users u WHERE u.balance > 0 ORDER BY u.balance",
            "SELECT * FROM active_users WHERE role = 'admin'",
            "SELECT user_id, order_count, revenue FROM order_totals WHERE revenue > 1000",
            "SELECT a.name, t.revenue FROM active_users a JOIN order_totals t ON t.user_id = a.id",
        ],
    );
}

// ===========================================================================
// VALID: joins
// ===========================================================================

#[test]
fn valid_joins() {
    assert_all_valid(
        "joins",
        &[
            "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id",
            "SELECT u.name, o.total FROM users u INNER JOIN orders o ON o.user_id = u.id AND o.total > 10",
            "SELECT u.name, o.id FROM users u LEFT JOIN orders o ON o.user_id = u.id WHERE o.id IS NULL",
            "SELECT u.name, o.id FROM users u LEFT OUTER JOIN orders o ON o.user_id = u.id",
            "SELECT u.name, o.id FROM orders o RIGHT JOIN users u ON o.user_id = u.id",
            "SELECT u.name, o.id FROM users u FULL OUTER JOIN orders o ON o.user_id = u.id",
            "SELECT u.name, p.title FROM users u CROSS JOIN products p",
            "SELECT u.name, p.title FROM users u, products p WHERE u.id = p.id",
            "SELECT order_id, product_id, quantity FROM order_items JOIN products ON products.id = order_items.product_id",
            "SELECT e.name AS employee, m.name AS manager FROM users e LEFT JOIN users m ON m.id = e.manager_id",
            "SELECT o.id, i.quantity, p.title FROM orders o JOIN order_items i ON i.order_id = o.id JOIN products p ON p.id = i.product_id",
            "SELECT * FROM orders o JOIN billing.invoices inv ON inv.order_id = o.id",
            "SELECT c.name, p.name FROM categories c LEFT JOIN categories p ON p.id = c.parent_id",
            "SELECT u.id, latest.placed_at FROM users u CROSS JOIN LATERAL (SELECT o.placed_at FROM orders o WHERE o.user_id = u.id ORDER BY o.placed_at DESC LIMIT 1) latest",
            "SELECT u.id, latest.total FROM users u LEFT JOIN LATERAL (SELECT total FROM orders WHERE orders.user_id = u.id LIMIT 3) AS latest ON true",
            "SELECT u.id, x.cnt FROM users u, LATERAL (SELECT count(*) AS cnt FROM orders o WHERE o.user_id = u.id) x",
            "SELECT * FROM (SELECT id AS order_id FROM orders) a NATURAL JOIN order_items",
            "SELECT o.id, u.email FROM orders o JOIN users u ON (u.id = o.user_id)",
            "SELECT * FROM users u JOIN orders o ON u.id = o.user_id JOIN billing.invoices i ON i.order_id = o.id WHERE i.paid",
        ],
    );
}

// ===========================================================================
// VALID: subqueries
// ===========================================================================

#[test]
fn valid_subqueries() {
    assert_all_valid(
        "subqueries",
        &[
            "SELECT name, (SELECT count(*) FROM orders o WHERE o.user_id = u.id) AS n FROM users u",
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)",
            "SELECT * FROM users WHERE id NOT IN (SELECT user_id FROM orders WHERE status = 'cancelled')",
            "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)",
            "SELECT * FROM users u WHERE NOT EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)",
            "SELECT * FROM products WHERE price > ALL (SELECT unit_price FROM order_items)",
            "SELECT * FROM products WHERE price = ANY (SELECT unit_price FROM order_items)",
            "SELECT * FROM products WHERE price > (SELECT avg(price) FROM products)",
            "SELECT * FROM orders o WHERE total > (SELECT avg(total) FROM orders o2 WHERE o2.user_id = o.user_id)",
            "SELECT t.uid, t.cnt FROM (SELECT user_id AS uid, count(*) AS cnt FROM orders GROUP BY user_id) t WHERE t.cnt > 2",
            "SELECT x.a, x.b FROM (SELECT id, name FROM users) AS x(a, b)",
            "SELECT a FROM (SELECT id FROM users) AS x(a) WHERE a > 3",
            "SELECT * FROM (SELECT * FROM users) sub WHERE sub.is_active",
            "SELECT * FROM users WHERE (SELECT max(total) FROM orders WHERE orders.user_id = users.id) > 100",
            "SELECT u.id FROM users u WHERE u.id IN (SELECT o.user_id FROM orders o WHERE EXISTS (SELECT 1 FROM order_items i WHERE i.order_id = o.id AND i.quantity > u.id))",
            "SELECT (SELECT name FROM users WHERE id = 1) AS first_name",
            "SELECT * FROM orders WHERE user_id = (SELECT id FROM users WHERE email = 'a@b.c')",
        ],
    );
}

// ===========================================================================
// VALID: CTEs
// ===========================================================================

#[test]
fn valid_ctes() {
    assert_all_valid(
        "ctes",
        &[
            "WITH big AS (SELECT id, total FROM orders WHERE total > 100) SELECT id, total FROM big",
            "WITH big AS (SELECT * FROM orders WHERE total > 100) SELECT count(*) FROM big",
            "WITH a AS (SELECT id FROM users), b AS (SELECT user_id FROM orders) SELECT * FROM a JOIN b ON b.user_id = a.id",
            "WITH a AS (SELECT id, name FROM users), b AS (SELECT id FROM a WHERE name LIKE 'A%') SELECT id FROM b",
            "WITH t(x, y) AS (SELECT id, name FROM users) SELECT x, y FROM t",
            "WITH RECURSIVE tree AS (SELECT id, parent_id, name, 1 AS depth FROM categories WHERE parent_id IS NULL UNION ALL SELECT c.id, c.parent_id, c.name, t.depth + 1 FROM categories c JOIN tree t ON c.parent_id = t.id) SELECT * FROM tree ORDER BY depth",
            "WITH RECURSIVE nums(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM nums WHERE n < 10) SELECT n FROM nums",
            "WITH RECURSIVE chain AS (SELECT id, manager_id FROM users WHERE id = $1 UNION SELECT u.id, u.manager_id FROM users u JOIN chain c ON u.id = c.manager_id) SELECT id FROM chain",
            "WITH upd AS (UPDATE products SET stock = stock - 1 WHERE id = 1 RETURNING id, stock) SELECT * FROM upd",
            "WITH ins AS (INSERT INTO categories (name) VALUES ('new') RETURNING id) SELECT id FROM ins",
            "WITH totals AS MATERIALIZED (SELECT user_id, sum(total) AS s FROM orders GROUP BY user_id) SELECT u.name, t.s FROM users u JOIN totals t ON t.user_id = u.id",
            "WITH x AS NOT MATERIALIZED (SELECT 1 AS one) SELECT one FROM x",
            "WITH o AS (SELECT id, user_id FROM orders) SELECT o.id FROM o JOIN users ON users.id = o.user_id",
        ],
    );
}

// ===========================================================================
// VALID: set operations
// ===========================================================================

#[test]
fn valid_set_operations() {
    assert_all_valid(
        "set operations",
        &[
            "SELECT id FROM users UNION SELECT user_id FROM orders",
            "SELECT id FROM users UNION ALL SELECT user_id FROM orders ORDER BY 1",
            "SELECT id FROM users INTERSECT SELECT user_id FROM orders",
            "SELECT id FROM users EXCEPT SELECT user_id FROM orders",
            "(SELECT id FROM users ORDER BY id LIMIT 5) UNION (SELECT user_id FROM orders LIMIT 5)",
            "SELECT * FROM (SELECT id FROM users UNION SELECT user_id FROM orders) ids(x) WHERE x > 1",
            "SELECT id FROM users UNION SELECT user_id FROM orders UNION SELECT order_id FROM billing.invoices",
        ],
    );
}

// ===========================================================================
// VALID: window functions and aggregates
// ===========================================================================

#[test]
fn valid_window_functions() {
    assert_all_valid(
        "window functions",
        &[
            "SELECT id, row_number() OVER (ORDER BY placed_at) FROM orders",
            "SELECT id, rank() OVER (PARTITION BY user_id ORDER BY total DESC) AS r FROM orders",
            "SELECT id, dense_rank() OVER w, sum(total) OVER w FROM orders WINDOW w AS (PARTITION BY user_id ORDER BY placed_at)",
            "SELECT id, sum(total) OVER (ORDER BY placed_at ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) FROM orders",
            "SELECT id, avg(total) OVER (PARTITION BY user_id ORDER BY placed_at ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING) FROM orders",
            "SELECT id, sum(total) OVER (ORDER BY placed_at RANGE BETWEEN INTERVAL '7 days' PRECEDING AND CURRENT ROW) FROM orders",
            "SELECT id, lag(total) OVER (ORDER BY id), lead(total, 2, 0) OVER (ORDER BY id) FROM orders",
            "SELECT id, first_value(total) OVER w, last_value(total) OVER w FROM orders WINDOW w AS (ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)",
            "SELECT id, ntile(4) OVER (ORDER BY total) FROM orders",
            "SELECT id, percent_rank() OVER (ORDER BY total), cume_dist() OVER (ORDER BY total) FROM orders",
            "SELECT * FROM (SELECT id, user_id, row_number() OVER (PARTITION BY user_id ORDER BY placed_at DESC) AS rn FROM orders) t WHERE rn = 1",
            "SELECT user_id, count(*) OVER () FROM orders",
            "SELECT id, count(*) FILTER (WHERE status = 'paid') OVER (PARTITION BY user_id) FROM orders",
        ],
    );
}

#[test]
fn valid_aggregates() {
    assert_all_valid(
        "aggregates",
        &[
            "SELECT count(*) FILTER (WHERE status = 'paid') AS paid, count(*) FILTER (WHERE status = 'pending') AS pending FROM orders",
            "SELECT user_id, string_agg(note, ', ' ORDER BY placed_at) FROM orders GROUP BY user_id",
            "SELECT user_id, array_agg(id ORDER BY placed_at DESC) FROM orders GROUP BY user_id",
            "SELECT array_agg(DISTINCT role) FROM users",
            "SELECT jsonb_agg(jsonb_build_object('id', id, 'name', name)) FROM users",
            "SELECT jsonb_object_agg(sku, price) FROM products",
            "SELECT min(price), max(price), avg(price), sum(stock) FROM products",
            "SELECT bool_and(is_active), bool_or(is_active) FROM users",
            "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY total) FROM orders",
            "SELECT mode() WITHIN GROUP (ORDER BY status) FROM orders",
            "SELECT status, user_id, sum(total) FROM orders GROUP BY GROUPING SETS ((status), (user_id), ())",
            "SELECT status, user_id, sum(total) FROM orders GROUP BY ROLLUP (status, user_id)",
            "SELECT status, user_id, sum(total) FROM orders GROUP BY CUBE (status, user_id)",
            "SELECT status, grouping(status), count(*) FROM orders GROUP BY ROLLUP (status)",
            "SELECT count(*) FILTER (WHERE total > 10) * 1.0 / count(*) FROM orders",
            "SELECT user_id FROM orders GROUP BY user_id HAVING bool_or(status = 'shipped')",
        ],
    );
}

// ===========================================================================
// VALID: expressions, casts, literals
// ===========================================================================

#[test]
fn valid_conditional_expressions() {
    assert_all_valid(
        "conditional expressions",
        &[
            "SELECT CASE WHEN total > 100 THEN 'big' WHEN total > 10 THEN 'medium' ELSE 'small' END FROM orders",
            "SELECT CASE status WHEN 'paid' THEN 1 WHEN 'pending' THEN 0 END FROM orders",
            "SELECT COALESCE(note, 'none') FROM orders",
            "SELECT COALESCE(shipped_at, placed_at) FROM orders",
            "SELECT NULLIF(stock, 0) FROM products",
            "SELECT GREATEST(price, 1), LEAST(stock, 100) FROM products",
            "SELECT id FROM orders WHERE COALESCE(total, 0) > 10",
            "SELECT id FROM orders WHERE CASE WHEN note IS NULL THEN false ELSE true END",
            "SELECT sum(CASE WHEN status = 'paid' THEN total ELSE 0 END) FROM orders",
        ],
    );
}

#[test]
fn valid_casts_and_literals() {
    assert_all_valid(
        "casts and literals",
        &[
            "SELECT id::text FROM users",
            "SELECT '42'::int + 1",
            "SELECT CAST(price AS INTEGER) FROM products",
            "SELECT CAST(id AS TEXT) || name FROM users",
            "SELECT price::numeric(8, 1) FROM products",
            "SELECT created_at::date FROM users",
            "SELECT * FROM users WHERE created_at::date = CURRENT_DATE",
            "SELECT * FROM users WHERE birth_date > DATE '2000-01-01'",
            "SELECT * FROM orders WHERE placed_at > TIMESTAMP '2024-01-01 00:00:00'",
            "SELECT * FROM orders WHERE placed_at > now() - INTERVAL '30 days'",
            "SELECT placed_at + INTERVAL '1 hour' FROM orders",
            "SELECT age(birth_date) FROM users",
            "SELECT * FROM events WHERE duration > INTERVAL '5 minutes'",
            "SELECT TRUE, FALSE, NULL",
            "SELECT E'line\\nbreak'",
            "SELECT 1.5e3, -7, 0.25",
            "SELECT uuid::text FROM users",
            "SELECT settings::text FROM users",
            "SELECT status::text FROM orders",
            "SELECT 'admin'::user_role",
            "SELECT * FROM users WHERE role = 'admin'::user_role",
            "SELECT ip::text FROM events",
        ],
    );
}

#[test]
fn valid_string_literal_coercions() {
    assert_all_valid(
        "string literal coercions",
        &[
            "SELECT * FROM users WHERE birth_date BETWEEN '1990-01-01' AND '1999-12-31'",
            "SELECT * FROM users WHERE uuid = 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'",
            "SELECT * FROM users WHERE role IN ('admin', 'member')",
            "UPDATE users SET uuid = 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11' WHERE id = 1",
            "SELECT * FROM users WHERE email = 'a@b.c'",
            "SELECT * FROM users WHERE name = 'Ann'",
            "SELECT * FROM billing.invoices WHERE currency = 'EUR'",
            "INSERT INTO billing.invoices (order_id, amount, currency, issued_on) VALUES (1, 5, 'EUR', CURRENT_DATE)",
            // Explicitly typed literals (the untyped-literal forms are a known
            // sqlsift false positive, see the coverage report).
            "SELECT * FROM users WHERE birth_date = DATE '1990-05-01'",
            "SELECT * FROM users WHERE birth_date = '1990-05-01'::date",
            "SELECT * FROM users WHERE settings = '{\"theme\": \"dark\"}'::jsonb",
            "SELECT * FROM users WHERE tags = '{a,b}'::text[]",
            "SELECT * FROM users WHERE role = 'admin'::user_role",
            "SELECT * FROM orders WHERE status <> 'cancelled'::order_status",
            "SELECT * FROM orders WHERE status::text = 'cancelled'",
            "SELECT * FROM products WHERE price = 19.99",
            "SELECT * FROM products WHERE stock > 5",
            "SELECT * FROM users WHERE is_active = true",
            "SELECT * FROM events WHERE duration = INTERVAL '1 hour'",
            "SELECT * FROM events WHERE ip IS NOT NULL",
            "SELECT * FROM billing.invoices WHERE issued_on >= DATE '2024-01-01'",
            "UPDATE users SET birth_date = DATE '2000-02-29' WHERE id = 1",
            "UPDATE users SET settings = '{}'::jsonb WHERE id = 1",
            "UPDATE users SET tags = ARRAY['x', 'y'] WHERE id = 1",
            "UPDATE users SET is_active = false WHERE id = 1",
            "UPDATE orders SET status = 'shipped'::order_status WHERE id = 1",
            "UPDATE users SET role = 'guest'::user_role WHERE id = 1",
        ],
    );
}

#[test]
fn valid_functions() {
    assert_all_valid(
        "functions",
        &[
            "SELECT lower(email), upper(name), length(name), trim(name) FROM users",
            "SELECT substring(name FROM 1 FOR 3), position('a' IN name) FROM users",
            "SELECT concat(name, ' <', email, '>'), concat_ws(',', name, email) FROM users",
            "SELECT replace(email, '@', ' at '), split_part(email, '@', 2) FROM users",
            "SELECT left(sku, 3), right(sku, 2), lpad(sku, 10, '0') FROM products",
            "SELECT regexp_replace(email, '@.*$', '') FROM users",
            "SELECT name || ' ' || email FROM users",
            "SELECT date_trunc('day', placed_at), extract(year FROM placed_at), date_part('month', placed_at) FROM orders",
            "SELECT EXTRACT(EPOCH FROM shipped_at - placed_at) FROM orders",
            "SELECT to_char(placed_at, 'YYYY-MM-DD') FROM orders",
            "SELECT now(), current_timestamp, clock_timestamp(), CURRENT_TIME, LOCALTIMESTAMP",
            "SELECT placed_at AT TIME ZONE 'UTC' FROM orders",
            "SELECT make_date(2024, 1, 1), to_timestamp(0)",
            "SELECT abs(balance), round(balance, 1), ceil(balance), floor(balance), mod(id, 2) FROM users",
            "SELECT power(2, 10), sqrt(16), random(), trunc(price) FROM products",
            "SELECT gen_random_uuid()",
            "SELECT md5(email) FROM users",
            "SELECT coalesce(array_length(tags, 1), 0) FROM users",
            "SELECT cardinality(tags) FROM users",
            "SELECT array_to_string(tags, ',') FROM users",
            "SELECT string_to_array('a,b', ',')",
            "SELECT jsonb_build_object('id', id) FROM users",
            "SELECT jsonb_typeof(settings), jsonb_array_length(payload) FROM users, events",
            "SELECT pg_typeof(id) FROM users",
        ],
    );
}

// ===========================================================================
// VALID: JSON and array operators
// ===========================================================================

#[test]
fn valid_json_operators() {
    assert_all_valid(
        "json operators",
        &[
            "SELECT settings -> 'theme' FROM users",
            "SELECT settings ->> 'theme' FROM users",
            "SELECT settings #> '{a,b}' FROM users",
            "SELECT settings #>> '{a,b}' FROM users",
            "SELECT * FROM users WHERE settings @> '{\"theme\": \"dark\"}'",
            "SELECT * FROM users WHERE '{\"theme\": \"dark\"}' <@ settings",
            "SELECT * FROM users WHERE settings ? 'theme'",
            "SELECT * FROM users WHERE settings ?| array['a', 'b']",
            "SELECT * FROM users WHERE settings ?& array['a', 'b']",
            "SELECT * FROM users WHERE settings ->> 'theme' = 'dark'",
            "SELECT * FROM events WHERE (payload ->> 'amount')::numeric > 100",
            "SELECT payload -> 'items' -> 0 ->> 'sku' FROM events",
            "SELECT settings || '{\"x\": 1}'::jsonb FROM users",
            "SELECT * FROM products WHERE attributes ->> 'color' IN ('red', 'blue')",
            "SELECT attributes -> 'dims' ->> 'w' AS w FROM products ORDER BY w",
        ],
    );
}

#[test]
fn valid_array_operators() {
    assert_all_valid(
        "array operators",
        &[
            "SELECT * FROM users WHERE 'vip' = ANY(tags)",
            "SELECT * FROM users WHERE 'vip' <> ALL(tags)",
            "SELECT * FROM users WHERE tags @> ARRAY['vip']",
            "SELECT * FROM users WHERE tags <@ ARRAY['vip', 'beta']",
            "SELECT * FROM users WHERE tags && ARRAY['vip']",
            "SELECT tags[1] FROM users",
            "SELECT tags[1:2] FROM users",
            "SELECT * FROM users WHERE tags[1] = 'vip'",
            "SELECT ARRAY[1, 2, 3]",
            "SELECT ARRAY[id, manager_id] FROM users",
            "SELECT tags || 'new'::text FROM users",
            "SELECT array_append(tags, 'x') FROM users",
            "SELECT * FROM products WHERE 5 = ANY(ratings)",
            "SELECT * FROM users WHERE id = ANY($1)",
            "SELECT * FROM users WHERE id = ANY(ARRAY[1, 2, 3])",
            "SELECT ARRAY(SELECT id FROM orders WHERE user_id = u.id) FROM users u",
        ],
    );
}

#[test]
fn valid_pattern_matching() {
    assert_all_valid(
        "pattern matching",
        &[
            "SELECT * FROM users WHERE email LIKE '%@example.com'",
            "SELECT * FROM users WHERE email NOT LIKE '%@spam.com'",
            "SELECT * FROM users WHERE name ILIKE 'jo%'",
            "SELECT * FROM users WHERE name NOT ILIKE 'jo%'",
            "SELECT * FROM users WHERE name SIMILAR TO '(A|B)%'",
            "SELECT * FROM users WHERE email ~ '^[a-z]+@'",
            "SELECT * FROM users WHERE email ~* '^[A-Z]+@'",
            "SELECT * FROM users WHERE email !~ 'spam'",
            "SELECT * FROM users WHERE email !~* 'SPAM'",
            "SELECT * FROM users WHERE email LIKE $1",
            "SELECT * FROM products WHERE sku LIKE 'AB\\_%' ESCAPE '\\'",
        ],
    );
}

// ===========================================================================
// VALID: positional parameters
// ===========================================================================

#[test]
fn valid_positional_parameters() {
    assert_all_valid(
        "positional parameters",
        &[
            "SELECT * FROM users WHERE id = $1",
            "SELECT * FROM users WHERE id = $1 AND email = $2",
            "SELECT * FROM users WHERE created_at BETWEEN $1 AND $2",
            "SELECT * FROM users WHERE birth_date > $1",
            "SELECT * FROM users WHERE uuid = $1",
            "SELECT * FROM users WHERE role = $1",
            "SELECT * FROM users WHERE is_active = $1",
            "SELECT * FROM users WHERE settings @> $1",
            "SELECT * FROM users WHERE id IN ($1, $2, $3)",
            "SELECT * FROM users LIMIT $1 OFFSET $2",
            "SELECT $1::int + id FROM users",
            "SELECT * FROM orders WHERE total > $1 * 2",
            "INSERT INTO categories (parent_id, name) VALUES ($1, $2)",
            "INSERT INTO users (uuid, email, name) VALUES ($1, $2, $3) RETURNING id",
            "UPDATE users SET name = $1, balance = $2 WHERE id = $3",
            "UPDATE users SET settings = $1 WHERE uuid = $2",
            "DELETE FROM users WHERE id = $1",
            "SELECT * FROM users u JOIN orders o ON o.user_id = u.id AND o.total > $1",
            "SELECT user_id FROM orders GROUP BY user_id HAVING count(*) > $1",
            "SELECT COALESCE($1, name) FROM users",
            "SELECT * FROM generate_series(1, $1) g",
        ],
    );
}

// ===========================================================================
// VALID: identifiers and schema qualification
// ===========================================================================

#[test]
fn valid_quoted_identifiers() {
    assert_all_valid(
        "quoted identifiers",
        &[
            "SELECT \"Id\", \"Action\" FROM \"AuditLog\"",
            "SELECT a.\"UserId\" FROM \"AuditLog\" a WHERE a.\"CreatedAt\" > now() - INTERVAL '1 day'",
            "SELECT u.name FROM users u JOIN \"AuditLog\" l ON l.\"UserId\" = u.id",
            "INSERT INTO \"AuditLog\" (\"UserId\", \"Action\") VALUES (1, 'login')",
            "UPDATE \"AuditLog\" SET \"Action\" = 'x' WHERE \"Id\" = 1",
            "SELECT \"id\", \"email\" FROM \"users\"",
            "SELECT u.\"name\" FROM users AS \"u\"",
            "SELECT id AS \"User Id\" FROM users ORDER BY \"User Id\"",
        ],
    );
}

#[test]
fn valid_schema_qualified_names() {
    assert_all_valid(
        "schema qualified names",
        &[
            "SELECT * FROM public.users",
            "SELECT id, email FROM public.users WHERE id = 1",
            "SELECT u.id FROM public.users u JOIN public.orders o ON o.user_id = u.id",
            "SELECT * FROM billing.invoices",
            "SELECT i.amount, i.currency FROM billing.invoices i WHERE i.paid = false",
            "SELECT invoices.amount FROM billing.invoices",
            "SELECT o.id, i.amount FROM orders o LEFT JOIN billing.invoices i ON i.order_id = o.id",
            "INSERT INTO billing.invoices (order_id, amount, issued_on) VALUES (1, 10.00, CURRENT_DATE)",
            "UPDATE billing.invoices SET paid = true WHERE id = 1",
            "DELETE FROM billing.invoices WHERE paid",
            "INSERT INTO public.categories (name) VALUES ('x')",
            "SELECT sum(amount) FROM billing.invoices GROUP BY currency",
        ],
    );
}

// ===========================================================================
// VALID: table-valued functions and VALUES
// ===========================================================================

#[test]
fn valid_table_functions() {
    assert_all_valid(
        "table functions",
        &[
            "SELECT g FROM generate_series(1, 10) AS g",
            "SELECT n FROM generate_series(1, 10) AS g(n)",
            "SELECT d::date FROM generate_series('2024-01-01'::date, '2024-12-31'::date, INTERVAL '1 month') AS d",
            "SELECT t FROM unnest(ARRAY['a', 'b']) AS t",
            "SELECT u.id, tag FROM users u, unnest(u.tags) AS tag",
            "SELECT u.id, t.tag FROM users u CROSS JOIN LATERAL unnest(u.tags) AS t(tag)",
            "SELECT e.id, item FROM events e, jsonb_array_elements(e.payload -> 'items') AS item",
            "SELECT item ->> 'sku' FROM events e CROSS JOIN LATERAL jsonb_array_elements(e.payload -> 'items') item",
            "SELECT x.a, x.b FROM jsonb_to_recordset('[{\"a\":1,\"b\":\"x\"}]'::jsonb) AS x(a int, b text)",
            "SELECT k, v FROM users, jsonb_each_text(settings) AS kv(k, v)",
            "SELECT * FROM unnest(ARRAY[1, 2], ARRAY['a', 'b']) AS t(n, s)",
            "SELECT ord, val FROM unnest(ARRAY['x', 'y']) WITH ORDINALITY AS t(val, ord)",
        ],
    );
}

#[test]
fn valid_values_lists() {
    assert_all_valid(
        "values lists",
        &[
            "VALUES (1, 'a'), (2, 'b')",
            "SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS v(id, label)",
            "SELECT v.id, v.label FROM (VALUES (1, 'a')) v(id, label)",
            "SELECT u.name, v.label FROM users u JOIN (VALUES (1, 'one'), (2, 'two')) AS v(id, label) ON v.id = u.id",
            "WITH v(id, qty) AS (VALUES (1, 5), (2, 7)) SELECT p.title, v.qty FROM products p JOIN v ON v.id = p.id",
            "UPDATE products p SET stock = v.qty FROM (VALUES (1, 5), (2, 7)) AS v(id, qty) WHERE p.id = v.id",
        ],
    );
}

// ===========================================================================
// VALID: INSERT
// ===========================================================================

#[test]
fn valid_inserts() {
    assert_all_valid(
        "inserts",
        &[
            "INSERT INTO categories (name) VALUES ('a')",
            "INSERT INTO categories (name, parent_id) VALUES ('a', NULL), ('b', 1), ('c', 2)",
            "INSERT INTO categories DEFAULT VALUES",
            "INSERT INTO orders (user_id, total) VALUES (1, 9.99)",
            "INSERT INTO orders (user_id, total, status) VALUES (1, 9.99, 'paid'::order_status)",
            "INSERT INTO order_items (order_id, product_id, quantity, unit_price) SELECT 1, id, 1, price FROM products WHERE id = 3",
            "INSERT INTO categories (name) SELECT title FROM products",
            "INSERT INTO categories (name) SELECT DISTINCT kind FROM events",
            "INSERT INTO products (sku, title, price) VALUES ('A1', 'Thing', 1.00) ON CONFLICT DO NOTHING",
            "INSERT INTO products (sku, title, price) VALUES ('A1', 'Thing', 1.00) ON CONFLICT (sku) DO NOTHING",
            "INSERT INTO products (sku, title, price) VALUES ('A1', 'Thing', 1.00) ON CONFLICT (sku) DO UPDATE SET price = EXCLUDED.price",
            "INSERT INTO products (sku, title, price) VALUES ('A1', 'Thing', 1.00) ON CONFLICT (sku) DO UPDATE SET price = EXCLUDED.price, title = EXCLUDED.title WHERE products.price <> EXCLUDED.price",
            "INSERT INTO products AS p (sku, title, price) VALUES ('A1', 'T', 1.00) ON CONFLICT (sku) DO UPDATE SET stock = p.stock + 1",
            "INSERT INTO products (sku, title, price) VALUES ('A1', 'T', 1) ON CONFLICT ON CONSTRAINT products_sku_key DO NOTHING",
            "INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES (1, 2, 3, 4.00) ON CONFLICT (order_id, product_id) DO UPDATE SET quantity = order_items.quantity + EXCLUDED.quantity",
            "INSERT INTO categories (name) VALUES ('x') RETURNING *",
            "INSERT INTO categories (name) VALUES ('x') RETURNING id, name AS n",
            "INSERT INTO users (uuid, email, name) VALUES (gen_random_uuid(), 'x@y.z', 'X') RETURNING id, created_at",
            "INSERT INTO users (uuid, email, name, tags) VALUES ($1, $2, $3, ARRAY['a', 'b'])",
            "INSERT INTO billing.invoices (order_id, amount, issued_on) SELECT id, total, placed_at::date FROM orders WHERE status = 'paid' RETURNING id",
            "WITH src AS (SELECT id, total FROM orders) INSERT INTO billing.invoices (order_id, amount, issued_on) SELECT id, total, CURRENT_DATE FROM src",
        ],
    );
}

// ===========================================================================
// VALID: UPDATE
// ===========================================================================

#[test]
fn valid_updates() {
    assert_all_valid(
        "updates",
        &[
            "UPDATE users SET name = 'x'",
            "UPDATE users SET name = 'x', email = 'y' WHERE id = 1",
            "UPDATE users SET balance = balance + 10 WHERE id = 1",
            "UPDATE users u SET name = upper(u.name) WHERE u.id = 1",
            "UPDATE users SET manager_id = NULL WHERE manager_id = id",
            "UPDATE users SET (name, email) = ('a', 'b') WHERE id = 1",
            "UPDATE users SET (name, email) = (SELECT name, email FROM users WHERE id = 2) WHERE id = 1",
            "UPDATE orders SET total = (SELECT sum(quantity * unit_price) FROM order_items WHERE order_items.order_id = orders.id)",
            "UPDATE products SET stock = stock - oi.quantity FROM order_items oi WHERE oi.product_id = products.id AND oi.order_id = 5",
            "UPDATE users SET is_active = false WHERE id = 1 RETURNING *",
            "UPDATE users SET is_active = false WHERE id = 1 RETURNING id, email",
            "UPDATE users SET tags = array_append(tags, 'vip') WHERE 'vip' <> ALL(tags)",
            "UPDATE users SET settings = settings || '{\"a\": 1}'",
            "UPDATE users SET settings = jsonb_set(settings, '{theme}', '\"dark\"')",
            "UPDATE users SET role = CASE WHEN balance > 1000 THEN 'admin'::user_role ELSE role END",
            "WITH t AS (SELECT user_id, sum(total) AS s FROM orders GROUP BY user_id) UPDATE users SET balance = t.s FROM t WHERE t.user_id = users.id",
        ],
    );
}

// ===========================================================================
// VALID: DELETE
// ===========================================================================

#[test]
fn valid_deletes() {
    assert_all_valid(
        "deletes",
        &[
            "DELETE FROM events",
            "DELETE FROM events WHERE occurred_at < now() - INTERVAL '90 days'",
            "DELETE FROM orders o WHERE o.status = 'cancelled'::order_status",
            "DELETE FROM order_items USING orders WHERE orders.id = order_items.order_id AND orders.note IS NULL",
            "DELETE FROM order_items oi USING orders o, users u WHERE o.id = oi.order_id AND u.id = o.user_id AND NOT u.is_active",
            "DELETE FROM orders WHERE id = 1 RETURNING *",
            "DELETE FROM orders WHERE id = 1 RETURNING id, total",
            "DELETE FROM users WHERE id IN (SELECT user_id FROM orders WHERE total = 0)",
            "DELETE FROM users u WHERE NOT EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)",
        ],
    );
}

#[test]
fn valid_multi_statement() {
    assert_all_valid(
        "multi statement",
        &[
            "SELECT 1; SELECT id FROM users; UPDATE users SET name = 'x' WHERE id = 1;",
            "BEGIN; UPDATE products SET stock = stock - 1 WHERE id = 1; COMMIT;",
            "SELECT id FROM users -- trailing comment",
            "/* leading */ SELECT id FROM users",
        ],
    );
}

#[test]
fn valid_scoping_and_ordering() {
    assert_all_valid(
        "scoping and ordering",
        &[
            // Inner scope wins over outer scope for unqualified names.
            "SELECT * FROM users WHERE EXISTS (SELECT 1 FROM orders WHERE id = 1)",
            "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE id = 1)",
            "SELECT * FROM users WHERE id IN (SELECT id FROM orders)",
            "SELECT id FROM orders WHERE id IN (SELECT order_id FROM billing.invoices WHERE id = 3)",
            "UPDATE users SET name = 'x' WHERE id IN (SELECT user_id FROM orders WHERE id = 5)",
            "DELETE FROM users WHERE id IN (SELECT user_id FROM orders WHERE id = 5)",
            "WITH x AS (SELECT id FROM orders) SELECT id FROM users WHERE id IN (SELECT id FROM x)",
            // Outer references from correlated subqueries.
            "SELECT * FROM users u WHERE u.id IN (SELECT user_id FROM orders WHERE total > 0 AND email IS NOT NULL)",
            "SELECT name, (SELECT max(id) FROM orders WHERE user_id = users.id) FROM users",
            "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id AND note = name)",
            "SELECT u.id FROM users u WHERE u.id = 1 AND EXISTS (SELECT 1 FROM orders o JOIN order_items i ON i.order_id = o.id WHERE quantity > 1)",
            "SELECT (SELECT id FROM orders LIMIT 1) FROM users",
            "SELECT * FROM users u WHERE u.id = ALL (SELECT o.user_id FROM orders o)",
            "SELECT * FROM users u WHERE u.id NOT IN (SELECT DISTINCT user_id FROM orders)",
            "INSERT INTO categories (name) SELECT name FROM categories WHERE parent_id IS NULL",
            // ORDER BY variations.
            "SELECT o.id, o.total FROM orders o ORDER BY total",
            "SELECT id, total AS t FROM orders ORDER BY t",
            "SELECT count(*) AS c FROM orders HAVING count(*) > 1 ORDER BY c",
            "SELECT id FROM users ORDER BY name || email",
            "SELECT id FROM users ORDER BY lower(name)",
            "SELECT u.name FROM users u ORDER BY u.created_at DESC",
            "SELECT DISTINCT role FROM users ORDER BY role",
            "SELECT o.user_id, sum(o.total) FROM orders o GROUP BY o.user_id ORDER BY sum(o.total) DESC",
            // Fully-qualified column references.
            "SELECT \"AuditLog\".\"Id\" FROM \"AuditLog\"",
            "SELECT a.\"Id\" FROM public.\"AuditLog\" a",
            "SELECT * FROM \"AuditLog\" WHERE \"Action\" = 'x' ORDER BY \"CreatedAt\" DESC",
            "SELECT billing.invoices.amount FROM billing.invoices",
            "SELECT public.users.id FROM public.users",
            "SELECT users.id FROM public.users",
            "SELECT * FROM billing.invoices JOIN orders ON orders.id = invoices.order_id",
            "SELECT oi.product_id FROM order_items oi JOIN order_items oi2 USING (order_id)",
        ],
    );
}

#[test]
fn valid_type_compatible_expressions() {
    assert_all_valid(
        "type compatible expressions",
        &[
            "SELECT * FROM orders WHERE shipped_at > placed_at",
            "SELECT * FROM users u JOIN orders o ON o.placed_at = u.created_at",
            "SELECT sum(price * stock) FROM products",
            "SELECT price * quantity FROM products p JOIN order_items i ON i.product_id = p.id",
            "SELECT * FROM products WHERE price > 10.5",
            "SELECT * FROM order_items WHERE quantity * unit_price > 100",
            "SELECT * FROM orders WHERE user_id = 2.5",
            "SELECT id FROM users WHERE id = $1::bigint",
            "SELECT * FROM products WHERE stock % 2 = 0",
            "SELECT * FROM products WHERE -stock < 0",
            "SELECT * FROM users WHERE length(name) > 3",
            "SELECT * FROM users WHERE upper(name) = 'X'",
            "SELECT * FROM orders WHERE total > (SELECT avg(total) FROM orders) * 1.5",
            "SELECT * FROM users WHERE lower(email) = lower($1)",
            "SELECT * FROM users WHERE COALESCE(manager_id, 0) = 0",
            "SELECT * FROM orders WHERE extract(year FROM placed_at) = 2024",
            "SELECT * FROM orders WHERE date_trunc('day', placed_at) = date_trunc('day', now())",
            "SELECT * FROM users WHERE id::text = '1'",
            "SELECT * FROM users WHERE CAST(id AS TEXT) = '1'",
            "SELECT * FROM orders WHERE total::int > 5",
            "SELECT * FROM users WHERE id = '1'::bigint",
            "SELECT * FROM users WHERE created_at::date = '2024-01-01'::date",
            "SELECT * FROM users WHERE settings->>'n' = 'x'",
            "SELECT * FROM users WHERE (settings->>'n')::int > 3",
            "SELECT * FROM users WHERE CASE WHEN id > 1 THEN 'a' ELSE 'b' END = 'a'",
            "SELECT count(*) > 1 FROM users",
            "SELECT * FROM users WHERE is_active = (id > 3)",
            "SELECT * FROM products WHERE ratings[1] > 3",
            "SELECT * FROM users WHERE array_length(tags, 1) > 2",
            "SELECT 1 FROM users WHERE balance = 0.00",
            "SELECT * FROM users WHERE manager_id = id",
            "SELECT * FROM order_items i JOIN orders o ON o.id = i.order_id AND i.product_id = o.user_id",
            "INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES (1, 2, 3, 4)",
            "UPDATE orders SET user_id = 1.5",
            "UPDATE products SET price = stock * 2",
        ],
    );
}

// ===========================================================================
// INVALID: E0001 table not found
// ===========================================================================

#[test]
fn invalid_table_not_found() {
    use DiagnosticKind::TableNotFound as T;
    assert_all_error(
        "E0001",
        &[
            ("SELECT * FROM user_accounts", T),
            ("SELECT * FROM users WHERE is_active AND u.id = 1", T),
            ("SELECT x.id FROM users u", T),
            ("SELECT * FROM users u JOIN orders o ON o.user_id = x.id", T),
            ("SELECT * FROM users u JOIN ordrs o ON o.user_id = u.id", T),
            (
                "SELECT * FROM users u LEFT JOIN payments p ON p.user_id = u.id",
                T,
            ),
            (
                "SELECT * FROM users WHERE id IN (SELECT user_id FROM purchases)",
                T,
            ),
            (
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM missing m WHERE m.id = u.id)",
                T,
            ),
            ("SELECT (SELECT count(*) FROM nope) FROM users", T),
            ("SELECT * FROM (SELECT * FROM nope) t", T),
            ("WITH a AS (SELECT * FROM nope) SELECT * FROM a", T),
            ("SELECT id FROM users UNION SELECT id FROM nope", T),
            ("INSERT INTO nope (a) VALUES (1)", T),
            ("INSERT INTO categories (name) SELECT title FROM nope", T),
            ("UPDATE nope SET a = 1", T),
            (
                "UPDATE orders SET status = 'paid' FROM nope WHERE nope.id = orders.id",
                T,
            ),
            ("DELETE FROM nope", T),
            ("DELETE FROM orders USING nope WHERE nope.id = orders.id", T),
            ("SELECT * FROM billing.payments", T),
            ("SELECT * FROM billing.users", T),
            ("SELECT * FROM public.invoices", T),
            ("SELECT * FROM auditlog", T),
            ("SELECT * FROM \"Users\"", T),
        ],
    );
}

// ===========================================================================
// INVALID: E0002 column not found
// ===========================================================================

#[test]
fn invalid_column_not_found() {
    use DiagnosticKind::ColumnNotFound as C;
    assert_all_error(
        "E0002",
        &[
            ("SELECT emial FROM users", C),
            ("SELECT u.emial FROM users u", C),
            ("SELECT users.nope FROM users", C),
            ("SELECT * FROM users WHERE nope = 1", C),
            ("SELECT * FROM users WHERE is_active AND nope = 1", C),
            (
                "SELECT * FROM users u JOIN orders o ON o.customer_id = u.id",
                C,
            ),
            (
                "SELECT * FROM users u JOIN orders o ON o.user_id = u.uid",
                C,
            ),
            ("SELECT role, count(*) FROM users GROUP BY rol", C),
            ("SELECT id FROM users ORDER BY nope", C),
            (
                "SELECT role FROM users GROUP BY role HAVING max(nope) > 1",
                C,
            ),
            (
                "SELECT * FROM users WHERE id IN (SELECT nope FROM orders)",
                C,
            ),
            (
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.nope = u.id)",
                C,
            ),
            ("SELECT (SELECT max(nope) FROM orders) FROM users", C),
            ("SELECT t.nope FROM (SELECT id FROM users) t", C),
            ("SELECT t.id FROM (SELECT id AS uid FROM users) t", C),
            ("WITH a AS (SELECT nope FROM users) SELECT * FROM a", C),
            ("WITH a AS (SELECT id FROM users) SELECT name FROM a", C),
            ("WITH a(x) AS (SELECT id FROM users) SELECT id FROM a", C),
            (
                "SELECT id, sum(total) OVER (PARTITION BY nope) FROM orders",
                C,
            ),
            (
                "SELECT id, row_number() OVER (ORDER BY nope) FROM orders",
                C,
            ),
            ("SELECT count(*) FILTER (WHERE nope = 1) FROM orders", C),
            ("SELECT CASE WHEN nope THEN 1 END FROM users", C),
            ("SELECT COALESCE(nope, 1) FROM users", C),
            ("SELECT lower(nope) FROM users", C),
            ("SELECT CAST(nope AS TEXT) FROM users", C),
            ("SELECT nope::text FROM users", C),
            (
                "INSERT INTO users (uuid, emial, name) VALUES ($1, $2, $3)",
                C,
            ),
            ("UPDATE users SET emial = 'x'", C),
            ("UPDATE users SET name = 'x' WHERE nope = 1", C),
            ("UPDATE users SET name = nope", C),
            ("DELETE FROM users WHERE nope = 1", C),
            ("SELECT * FROM billing.invoices WHERE total > 1", C),
            ("SELECT i.total FROM billing.invoices i", C),
            ("SELECT nope FROM active_users", C),
            ("SELECT is_active FROM active_users", C),
            ("SELECT total FROM order_totals", C),
            ("SELECT id FROM users UNION SELECT nope FROM orders", C),
            ("SELECT x FROM (VALUES (1, 2)) AS v(a, b)", C),
            ("SELECT n FROM generate_series(1, 3) AS g(m)", C),
        ],
    );
}

// ===========================================================================
// INVALID: E0003 type mismatch
// ===========================================================================

#[test]
fn invalid_type_mismatch() {
    use DiagnosticKind::TypeMismatch as M;
    assert_all_error(
        "E0003",
        &[
            ("SELECT * FROM users WHERE id = 'abc'", M),
            ("SELECT * FROM users WHERE is_active = 42", M),
            ("SELECT * FROM users WHERE id = true", M),
            ("SELECT * FROM users WHERE birth_date = 5", M),
            ("SELECT * FROM users WHERE balance > 'lots'", M),
            ("SELECT * FROM users WHERE name = id", M),
            ("SELECT * FROM users WHERE id + 'x' > 1", M),
            ("SELECT * FROM orders WHERE total * true > 1", M),
            ("SELECT * FROM users WHERE created_at = 1", M),
            ("SELECT * FROM products WHERE stock = 'many'", M),
            ("INSERT INTO categories (id, name) VALUES ('one', 'x')", M),
            (
                "INSERT INTO products (sku, title, price) VALUES ('a', 'b', 'cheap')",
                M,
            ),
            (
                "INSERT INTO users (uuid, email, name, is_active) VALUES ($1, $2, $3, 5)",
                M,
            ),
            ("UPDATE users SET id = 'text'", M),
            ("UPDATE products SET stock = 'lots'", M),
            ("UPDATE users SET is_active = 1", M),
            ("UPDATE users SET name = 'x' WHERE id = 'y'", M),
            ("DELETE FROM users WHERE id = 'y'", M),
            ("SELECT * FROM billing.invoices WHERE amount = 'x'", M),
            ("SELECT * FROM billing.invoices WHERE paid = 3", M),
        ],
    );
}

// ===========================================================================
// INVALID: E0005 column count mismatch
// ===========================================================================

#[test]
fn invalid_column_count_mismatch() {
    use DiagnosticKind::ColumnCountMismatch as N;
    assert_all_error(
        "E0005",
        &[
            ("INSERT INTO categories (name) VALUES ('a', 'b')", N),
            ("INSERT INTO categories (name, parent_id) VALUES ('a')", N),
            ("INSERT INTO categories (name) VALUES ('a'), ('b', 1)", N),
            (
                "INSERT INTO billing.invoices (order_id, amount) VALUES (1)",
                N,
            ),
            ("INSERT INTO categories VALUES (1, 2, 'x', 'extra')", N),
        ],
    );
}

// ===========================================================================
// INVALID: E0006 ambiguous column
// ===========================================================================

#[test]
fn invalid_ambiguous_column() {
    use DiagnosticKind::AmbiguousColumn as A;
    assert_all_error(
        "E0006",
        &[
            ("SELECT id FROM users u JOIN orders o ON o.user_id = u.id", A),
            ("SELECT u.name FROM users u JOIN orders o ON o.user_id = u.id WHERE id = 1", A),
            ("SELECT u.name FROM users u JOIN orders o ON o.user_id = u.id ORDER BY id", A),
            ("SELECT count(*) FROM users u JOIN orders o ON o.user_id = u.id GROUP BY id", A),
            ("SELECT u.name FROM users u JOIN orders o ON id = o.user_id", A),
            ("SELECT u.name FROM users u, products p WHERE id = 1", A),
            ("SELECT e.name FROM users e JOIN users m ON m.id = e.manager_id WHERE email = 'x'", A),
            ("SELECT * FROM orders o JOIN billing.invoices i ON i.order_id = o.id WHERE id = 1", A),
            ("SELECT order_id FROM order_items oi JOIN billing.invoices i ON i.order_id = oi.order_id", A),
            ("WITH a AS (SELECT id FROM users), b AS (SELECT id FROM orders) SELECT id FROM a, b", A),
            ("SELECT name FROM categories c JOIN users u ON u.id = c.id", A),
        ],
    );
}

// ===========================================================================
// INVALID: E0007 JOIN type mismatch
// ===========================================================================

#[test]
fn invalid_join_type_mismatch() {
    use DiagnosticKind::JoinTypeMismatch as J;
    assert_all_error(
        "E0007",
        &[
            ("SELECT * FROM users u JOIN orders o ON u.email = o.id", J),
            (
                "SELECT * FROM users u JOIN orders o ON u.is_active = o.user_id",
                J,
            ),
            (
                "SELECT * FROM users u LEFT JOIN orders o ON o.user_id = u.created_at",
                J,
            ),
            (
                "SELECT * FROM orders o JOIN billing.invoices i ON i.issued_on = o.id",
                J,
            ),
            (
                "SELECT * FROM products p JOIN order_items i ON i.product_id = p.sku",
                J,
            ),
            (
                "SELECT * FROM users u JOIN orders o ON o.user_id = u.id AND o.note = u.id",
                J,
            ),
        ],
    );
}

// ===========================================================================
// INVALID: spans, help, and messages
// ===========================================================================

#[test]
fn span_and_help_table_not_found() {
    assert_single(
        "SELECT 1 FROM userz",
        DiagnosticKind::TableNotFound,
        1,
        15,
        "userz",
        Some("Did you mean 'users'?"),
    );
    assert_single(
        "SELECT 1\nFROM ordres o",
        DiagnosticKind::TableNotFound,
        2,
        6,
        "ordres",
        Some("Did you mean 'orders'?"),
    );
    assert_single(
        "SELECT 1 FROM zzzzzz",
        DiagnosticKind::TableNotFound,
        1,
        15,
        "zzzzzz",
        Some("Check that the table exists"),
    );
}

#[test]
fn span_and_help_column_not_found() {
    assert_single(
        "SELECT emial FROM users",
        DiagnosticKind::ColumnNotFound,
        1,
        8,
        "emial",
        Some("email"),
    );
    assert_single(
        "SELECT id\nFROM users\nWHERE is_actve",
        DiagnosticKind::ColumnNotFound,
        3,
        7,
        "is_actve",
        Some("is_active"),
    );
    assert_single(
        "SELECT u.id FROM users u WHERE u.balanse > 0",
        DiagnosticKind::ColumnNotFound,
        1,
        34,
        "balanse",
        Some("balance"),
    );
    assert_single(
        "UPDATE users SET nmae = 'x'",
        DiagnosticKind::ColumnNotFound,
        1,
        18,
        "nmae",
        Some("name"),
    );
    assert_single(
        "INSERT INTO categories (nam) VALUES ('x')",
        DiagnosticKind::ColumnNotFound,
        1,
        25,
        "nam",
        Some("name"),
    );
}

#[test]
fn span_type_mismatch() {
    let catalog = catalog();
    let sql = "SELECT id\nFROM users\nWHERE id = 'abc'";
    let diags = analyze(&catalog, sql);
    assert_eq!(diags.len(), 1, "{}", fmt_diags(&diags));
    assert_eq!(diags[0].kind, DiagnosticKind::TypeMismatch);
    let span = diags[0].span.expect("span");
    assert_eq!(span.line, 3, "{}", fmt_diags(&diags));
}

#[test]
fn span_ambiguous_column() {
    let catalog = catalog();
    let sql = "SELECT u.name\nFROM users u JOIN orders o ON o.user_id = u.id\nWHERE id = 1";
    let diags = analyze(&catalog, sql);
    assert_eq!(diags.len(), 1, "{}", fmt_diags(&diags));
    assert_eq!(diags[0].kind, DiagnosticKind::AmbiguousColumn);
    let span = diags[0].span.expect("span");
    assert_eq!((span.line, span.column), (3, 7), "{}", fmt_diags(&diags));
}

#[test]
fn multiple_errors_reported() {
    let catalog = catalog();
    let diags = analyze(&catalog, "SELECT nope1, nope2 FROM users WHERE nope3 = 1");
    let n = diags
        .iter()
        .filter(|d| d.kind == DiagnosticKind::ColumnNotFound)
        .count();
    assert_eq!(n, 3, "{}", fmt_diags(&diags));
}

#[test]
fn parse_error_reported() {
    let catalog = catalog();
    for sql in [
        "SELECT FROM WHERE",
        "SELEC id FROM users",
        "SELECT id FROM users WHERE (",
    ] {
        let diags = analyze(&catalog, sql);
        assert!(
            diags.iter().any(|d| d.kind == DiagnosticKind::ParseError),
            "SQL: {sql}\n{}",
            fmt_diags(&diags)
        );
    }
}

// ===========================================================================
// INVALID: column-not-found in less common positions
// ===========================================================================

#[test]
fn invalid_column_not_found_expression_positions() {
    use DiagnosticKind::ColumnNotFound as C;
    assert_all_error(
        "E0002 expression positions",
        &[
            ("SELECT id FROM users ORDER BY nope NULLS LAST", C),
            ("SELECT u.id FROM users u CROSS JOIN LATERAL (SELECT o.nope FROM orders o WHERE o.user_id = u.id) x", C),
            ("SELECT u.id FROM users u JOIN LATERAL (SELECT 1 AS one) x ON x.two = 1", C),
            ("SELECT id FROM users WHERE id = ANY(SELECT nope FROM orders)", C),
            ("SELECT id FROM users WHERE id = ANY(nope)", C),
            ("SELECT nope[1] FROM users", C),
            ("SELECT tags[nope] FROM users", C),
            ("SELECT settings ->> nope FROM users", C),
            ("SELECT * FROM users WHERE nope ILIKE 'x'", C),
            ("SELECT * FROM users WHERE email ~ nope", C),
            ("SELECT * FROM users WHERE id BETWEEN nope AND 3", C),
            ("SELECT * FROM users WHERE nope IS NULL", C),
            ("SELECT * FROM users WHERE nope IS DISTINCT FROM 1", C),
            ("SELECT EXTRACT(year FROM nope) FROM users", C),
            ("SELECT nope AT TIME ZONE 'UTC' FROM users", C),
            ("SELECT ARRAY[nope] FROM users", C),
            ("SELECT string_agg(nope, ',' ORDER BY id) FROM users", C),
            ("SELECT status FROM orders GROUP BY ROLLUP (nope)", C),
            ("SELECT status FROM orders GROUP BY GROUPING SETS ((nope), ())", C),
            ("SELECT tag FROM users, unnest(tags) AS t(tag) WHERE nope = 1", C),
            ("SELECT x.c FROM jsonb_to_recordset('[]'::jsonb) AS x(a int, b text)", C),
            ("WITH RECURSIVE t AS (SELECT id FROM categories UNION ALL SELECT c.id FROM categories c JOIN t ON c.nope = t.id) SELECT * FROM t", C),
            ("WITH a AS (SELECT 1 AS x) SELECT a.y FROM a", C),
            ("UPDATE users u SET name = 'x' FROM orders o WHERE o.nope = u.id", C),
            ("UPDATE users SET name = (SELECT nope FROM orders LIMIT 1)", C),
            ("DELETE FROM order_items oi USING orders o WHERE o.nope = oi.order_id", C),
            ("INSERT INTO categories (name) SELECT nope FROM products", C),
            ("INSERT INTO categories (name) VALUES (nope)", C),
            // Output aliases are not visible in WHERE (PostgreSQL rejects this).
            ("SELECT total AS amount FROM orders WHERE amount > 1", C),
            // A column from a different table is not visible.
            ("SELECT 1 FROM users WHERE balance > stock", C),
        ],
    );
}

#[test]
fn invalid_ambiguous_more_positions() {
    use DiagnosticKind::AmbiguousColumn as A;
    assert_all_error(
        "E0006 more positions",
        &[
            ("SELECT id FROM orders o JOIN billing.invoices i ON i.order_id = o.id", A),
            ("SELECT * FROM users u JOIN orders o ON o.user_id = u.id WHERE status = 'paid'::order_status AND id > 1", A),
            ("SELECT max(id) FROM users u JOIN orders o ON o.user_id = u.id", A),
            ("SELECT u.id FROM users u JOIN orders o ON o.user_id = u.id HAVING count(id) > 1", A),
        ],
    );
}

#[test]
fn invalid_type_mismatch_more_positions() {
    use DiagnosticKind::TypeMismatch as M;
    assert_all_error(
        "E0003 more positions",
        &[
            ("SELECT * FROM users WHERE id = 'abc' OR name = 'x'", M),
            ("SELECT * FROM users WHERE NOT (id = 'abc')", M),
            ("SELECT * FROM users WHERE (id + 1) * 2 = 'x'", M),
            ("SELECT * FROM users WHERE name > 3", M),
            ("SELECT * FROM users WHERE uuid = 5", M),
            ("SELECT * FROM users WHERE settings = 5", M),
            (
                "SELECT * FROM users WHERE is_active = 'x' AND id = false",
                M,
            ),
            ("INSERT INTO orders (user_id, total) VALUES (1, true)", M),
            (
                "INSERT INTO orders (user_id, total) VALUES (1, 'x'), (2, 3)",
                M,
            ),
            ("UPDATE orders SET total = true WHERE id = 1", M),
            ("UPDATE orders SET placed_at = 5", M),
            (
                "INSERT INTO users (uuid, email, name, birth_date) VALUES ($1, $2, $3, 42)",
                M,
            ),
        ],
    );
}

#[test]
fn invalid_join_type_mismatch_more() {
    use DiagnosticKind::JoinTypeMismatch as J;
    assert_all_error(
        "E0007 more",
        &[
            (
                "SELECT * FROM orders o JOIN billing.invoices i ON i.amount = o.placed_at",
                J,
            ),
            ("SELECT * FROM users u JOIN users m ON m.uuid = u.id", J),
            (
                "SELECT * FROM orders o LEFT JOIN users u ON u.email = o.total",
                J,
            ),
            (
                "SELECT * FROM orders o FULL JOIN users u ON u.is_active = o.id",
                J,
            ),
        ],
    );
}
