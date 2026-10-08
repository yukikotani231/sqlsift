// Regression tests for false positives and noisy diagnostics found by
// running real-world style queries against PostgreSQL, MySQL and SQLite schemas.
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const PG_SCHEMA: &str = r#"
    CREATE TYPE status AS ENUM ('active', 'inactive');
    CREATE TABLE users (
        id BIGSERIAL PRIMARY KEY,
        name VARCHAR(100) NOT NULL,
        email TEXT UNIQUE,
        status status NOT NULL DEFAULT 'active',
        metadata JSONB,
        tags TEXT[],
        is_admin BOOLEAN NOT NULL DEFAULT false,
        score REAL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at TIMESTAMP(3)
    );
    CREATE TABLE orders (
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT NOT NULL REFERENCES users(id),
        total NUMERIC(10, 2) NOT NULL,
        placed_on DATE,
        note VARCHAR(500)
    );
"#;

const MYSQL_SCHEMA: &str = r#"
    CREATE TABLE users (
        id INT NOT NULL AUTO_INCREMENT,
        name VARCHAR(100) NOT NULL,
        is_admin TINYINT(1) NOT NULL DEFAULT 0,
        created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (id)
    );
"#;

const SQLITE_SCHEMA: &str = r#"
    CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, active INTEGER);
"#;

fn catalog(schema: &str, dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    builder.build().0
}

fn analyze(schema: &str, dialect: SqlDialect, sql: &str) -> Vec<Diagnostic> {
    let catalog = catalog(schema, dialect);
    Analyzer::with_dialect(&catalog, dialect).analyze(sql)
}

fn assert_valid(schema: &str, dialect: SqlDialect, sql: &str) {
    let diagnostics = analyze(schema, dialect, sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics for `{sql}`, got: {diagnostics:#?}"
    );
}

/// Text covered by a diagnostic's (1-indexed line/column) span
fn span_text<'a>(sql: &'a str, diagnostic: &Diagnostic) -> &'a str {
    let span = diagnostic.span.expect("diagnostic should have a span");
    assert!(span.line > 0, "span should have a line: {span:?}");
    let line = sql.lines().nth(span.line - 1).unwrap();
    &line[span.column - 1..span.column - 1 + span.length]
}

fn kinds(diagnostics: &[Diagnostic]) -> Vec<DiagnosticKind> {
    diagnostics.iter().map(|d| d.kind).collect()
}

// ---------------------------------------------------------------------------
// String literals are untyped and coerce to the other operand's type
// ---------------------------------------------------------------------------

#[test]
fn string_literal_coerces_to_temporal_json_array_and_enum_types() {
    for sql in [
        "SELECT id FROM users WHERE created_at > '2024-01-01'",
        "SELECT id FROM orders WHERE placed_on = '2024-01-01'",
        "SELECT id FROM orders WHERE placed_on BETWEEN '2024-01-01' AND '2024-12-31'",
        "SELECT id FROM users WHERE status = 'active'",
        "DELETE FROM orders o USING users u WHERE o.user_id = u.id AND u.status = 'inactive'",
        "UPDATE users SET metadata = '{\"a\":1}' WHERE id = 1",
        "UPDATE users SET tags = '{a,b}' WHERE id = 1",
        "INSERT INTO orders (user_id, total, placed_on) VALUES (1, 10, '2024-01-01')",
        "INSERT INTO users (name, status) VALUES ('a', 'inactive')",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn string_literal_that_parses_as_number_or_boolean_is_accepted() {
    for sql in [
        "SELECT id FROM users WHERE id = '42'",
        "SELECT id FROM orders WHERE total > '10.50'",
        "SELECT id FROM users WHERE is_admin = 'true'",
        "UPDATE users SET is_admin = 'f' WHERE id = 1",
        "INSERT INTO orders (user_id, total) VALUES ('1', '9.99')",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn string_literal_that_cannot_be_coerced_is_still_reported() {
    for sql in [
        "SELECT id FROM users WHERE id = 'abc'",
        "UPDATE orders SET total = 'free' WHERE id = 1",
        "INSERT INTO orders (user_id, total) VALUES ('one', 1)",
        "SELECT id FROM users WHERE is_admin = 'maybe'",
    ] {
        let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert_eq!(
            kinds(&diagnostics),
            vec![DiagnosticKind::TypeMismatch],
            "for `{sql}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Type compatibility between parameterized / related types
// ---------------------------------------------------------------------------

#[test]
fn timestamptz_column_accepts_now() {
    assert_valid(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO users (name, created_at) VALUES ('a', now())",
    );
}

#[test]
fn timestamps_with_different_precision_or_time_zone_are_comparable() {
    for sql in [
        "SELECT id FROM users WHERE updated_at < now()",
        "SELECT id FROM users WHERE updated_at > created_at",
        "UPDATE users SET updated_at = now() WHERE id = 1",
        "SELECT o.id FROM orders o JOIN users u ON o.placed_on = u.created_at",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn related_numeric_and_string_types_are_comparable() {
    for sql in [
        "SELECT id FROM orders WHERE total = CAST(1 AS NUMERIC)",
        "SELECT id FROM users WHERE score > CAST(1 AS NUMERIC(5, 1))",
        "SELECT o.id FROM orders o JOIN users u ON o.note = u.name",
        "SELECT id FROM users WHERE name = CAST('x' AS VARCHAR(10))",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

// ---------------------------------------------------------------------------
// Date/time arithmetic
// ---------------------------------------------------------------------------

#[test]
fn date_time_arithmetic_is_not_a_type_error() {
    for sql in [
        "SELECT id FROM users WHERE created_at >= now() - '1 day'::interval",
        "SELECT id FROM users WHERE created_at > now() - interval '7 days'",
        "SELECT id FROM orders WHERE placed_on + 7 > current_date",
        "SELECT id FROM orders WHERE placed_on - interval '1 month' < now()",
        "SELECT now() - created_at FROM users",
        "SELECT current_date - placed_on FROM orders",
        "SELECT '1 hour'::interval * 2 FROM users",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn date_time_arithmetic_result_type_is_checked() {
    // timestamp - interval is still a timestamp: comparing it with an integer is wrong
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users WHERE now() - interval '1 day' = 5",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);

    // Arithmetic on text is still reported
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT name + 1 FROM users",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
}

// ---------------------------------------------------------------------------
// Identifier case
// ---------------------------------------------------------------------------

#[test]
fn unquoted_table_names_are_case_insensitive() {
    for sql in [
        "SELECT Name FROM Users",
        "SELECT U.id FROM USERS U JOIN Orders o ON o.user_id = U.id",
        "INSERT INTO Users (name) VALUES ('a')",
        "UPDATE USERS SET name = 'a' WHERE id = 1",
        "SELECT users.id, Users.name FROM Users WHERE USERS.id = 1",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

// ---------------------------------------------------------------------------
// Dialect-specific booleans
// ---------------------------------------------------------------------------

#[test]
fn mysql_booleans_are_tinyint() {
    for sql in [
        "SELECT id FROM users WHERE is_admin = TRUE",
        "UPDATE users SET is_admin = FALSE WHERE id = 1",
        "UPDATE users SET created_at = NOW() WHERE id = 1",
        "SELECT id FROM users WHERE created_at > '2024-01-01 00:00:00'",
    ] {
        assert_valid(MYSQL_SCHEMA, SqlDialect::MySQL, sql);
    }
}

#[test]
fn sqlite_booleans_are_integers() {
    assert_valid(
        SQLITE_SCHEMA,
        SqlDialect::SQLite,
        "SELECT id FROM users WHERE active = TRUE",
    );
}

#[test]
fn postgres_boolean_is_not_an_integer() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users WHERE id = TRUE",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
}

// ---------------------------------------------------------------------------
// Diagnostic quality: spans, cascades and suggestions
// ---------------------------------------------------------------------------

#[test]
fn not_null_violation_points_at_the_column_in_insert() {
    let sql = "SELECT 1;\nINSERT INTO users (email, name) VALUES ('a@example.com', NULL);";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::PotentialNullViolation]
    );
    assert_eq!(span_text(sql, &diagnostics[0]), "name");
}

#[test]
fn not_null_violation_points_at_the_column_in_update() {
    let sql = "SELECT 1;\nUPDATE users SET name = NULL WHERE id = 1;";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::PotentialNullViolation]
    );
    assert_eq!(span_text(sql, &diagnostics[0]), "name");
}

#[test]
fn insert_column_count_mismatch_has_a_span() {
    let sql = "INSERT INTO orders (user_id, total) VALUES (1)";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::ColumnCountMismatch]
    );
    assert!(diagnostics[0].span.is_some(), "E0005 should have a span");
}

#[test]
fn missing_table_does_not_cascade_into_column_errors() {
    for sql in [
        "SELECT id, total FROM ordrs WHERE id = 1",
        "SELECT o.id, o.total FROM ordrs o WHERE o.id = 1",
        "SELECT * FROM ordrs",
        "SELECT o.* FROM ordrs o",
        "UPDATE ordrs SET total = 0 WHERE id = 1",
        "DELETE FROM ordrs WHERE id = 1",
    ] {
        let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert_eq!(
            kinds(&diagnostics),
            vec![DiagnosticKind::TableNotFound],
            "for `{sql}`: {diagnostics:#?}"
        );
    }
}

#[test]
fn missing_table_does_not_make_known_columns_ambiguous() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT name FROM users u JOIN ordrs o ON o.user_id = u.id",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
}

#[test]
fn columns_of_known_tables_are_still_checked_next_to_a_missing_table() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT u.naem FROM users u JOIN ordrs o ON o.user_id = u.id",
    );
    // diagnostics are reported in source order
    assert_eq!(
        kinds(&diagnostics),
        vec![
            DiagnosticKind::ColumnNotFound,
            DiagnosticKind::TableNotFound
        ]
    );
}

#[test]
fn table_not_found_suggests_similar_table() {
    for sql in [
        "SELECT id FROM ordrs",
        "INSERT INTO ordrs (id) VALUES (1)",
        "UPDATE ordrs SET id = 1",
    ] {
        let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
        assert_eq!(
            diagnostics[0].help.as_deref(),
            Some("Did you mean 'orders'?"),
            "for `{sql}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Scoping: set operations, USING / NATURAL joins, LATERAL
// ---------------------------------------------------------------------------

#[test]
fn set_operation_branches_have_their_own_scope() {
    for sql in [
        "SELECT id FROM users UNION SELECT id FROM orders",
        "SELECT id FROM users UNION ALL SELECT id FROM orders ORDER BY id",
        "SELECT id, 'u' AS src FROM users UNION SELECT id, 'o' FROM orders ORDER BY src",
        "SELECT email FROM users EXCEPT SELECT note FROM orders",
        "SELECT * FROM (SELECT id FROM users INTERSECT SELECT user_id FROM orders) t WHERE t.id > 1",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn set_operation_order_by_unknown_column_is_reported() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users UNION SELECT id FROM orders ORDER BY nope",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn using_and_natural_join_columns_are_not_ambiguous() {
    for sql in [
        "SELECT a.id FROM orders a JOIN orders b USING (user_id)",
        "SELECT user_id, a.total FROM orders a JOIN orders b USING (user_id) ORDER BY user_id",
        "SELECT id FROM users JOIN orders USING (id) WHERE id > 1",
        "SELECT id, name FROM users NATURAL JOIN orders",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn using_unknown_column_is_reported() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT 1 FROM users JOIN orders USING (nope)",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn lateral_subquery_prefers_its_own_tables() {
    for sql in [
        "SELECT u.name, x.id FROM users u JOIN LATERAL (SELECT id FROM orders WHERE user_id = u.id) x ON true",
        "SELECT u.name FROM users u, LATERAL (SELECT id, total FROM orders o WHERE o.user_id = u.id LIMIT 1) x",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

// ---------------------------------------------------------------------------
// Type checks apply in every query block
// ---------------------------------------------------------------------------

#[test]
fn type_errors_are_reported_in_nested_query_blocks() {
    for sql in [
        "SELECT id FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.total = 'free')",
        "WITH a AS (SELECT id FROM users WHERE id = 'x') SELECT * FROM a",
        "SELECT * FROM (SELECT * FROM users WHERE is_admin = 1) t",
        "SELECT user_id FROM orders GROUP BY user_id HAVING count(*) > 'many'",
        "SELECT id FROM users WHERE id IN (SELECT user_id FROM orders WHERE total > 'x')",
        "SELECT (SELECT max(total) FROM orders WHERE user_id = 'abc') FROM users",
        "SELECT id FROM users UNION SELECT id FROM orders WHERE total = 'free'",
        "INSERT INTO orders (user_id, total) SELECT id, 0 FROM users WHERE id = 'abc'",
        "SELECT id FROM users WHERE id + 'x' > 1",
    ] {
        let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert_eq!(
            kinds(&diagnostics),
            vec![DiagnosticKind::TypeMismatch],
            "for `{sql}`: {diagnostics:#?}"
        );
    }
}

#[test]
fn nested_query_blocks_are_typed_with_their_own_tables() {
    for sql in [
        // `total` resolves to orders.total (numeric) inside the subquery
        "SELECT id FROM users WHERE id IN (SELECT user_id FROM orders WHERE total > 10)",
        // inner `id` is orders.id, outer `name` is users.name
        "SELECT name FROM users WHERE EXISTS (SELECT 1 FROM orders WHERE id = 1 AND name = 'x')",
        "WITH t AS (SELECT user_id, sum(total) AS s FROM orders GROUP BY user_id) SELECT * FROM t WHERE s > 10",
        "SELECT id + '1' FROM users",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL identifier case rules
// ---------------------------------------------------------------------------

const QUOTED_SCHEMA: &str = r#"
    CREATE TABLE "AuditLog" (id SERIAL PRIMARY KEY, "createdAt" TIMESTAMPTZ);
    CREATE TABLE Accounts (id SERIAL PRIMARY KEY);
"#;

#[test]
fn quoted_table_names_are_case_sensitive_in_postgres() {
    for sql in [
        r#"SELECT id FROM "AuditLog""#,
        r#"SELECT a.id FROM "AuditLog" a"#,
        // unquoted names fold to lowercase on both sides
        "SELECT id FROM accounts",
        "SELECT id FROM ACCOUNTS",
        r#"SELECT id FROM "accounts""#,
        "WITH Recent AS (SELECT id FROM accounts) SELECT id FROM recent",
    ] {
        assert_valid(QUOTED_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
    for sql in [
        "SELECT id FROM auditlog",
        "SELECT id FROM AuditLog",
        r#"SELECT id FROM "Accounts""#,
    ] {
        let diagnostics = analyze(QUOTED_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert_eq!(
            kinds(&diagnostics),
            vec![DiagnosticKind::TableNotFound],
            "for `{sql}`"
        );
    }
}

#[test]
fn table_names_are_case_insensitive_in_mysql_and_sqlite() {
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        let schema = "CREATE TABLE Accounts (id INTEGER PRIMARY KEY);";
        assert_valid(schema, dialect, "SELECT id FROM accounts");
        assert_valid(schema, dialect, "SELECT id FROM ACCOUNTS");
    }
}

// ---------------------------------------------------------------------------
// Name resolution gaps
// ---------------------------------------------------------------------------

#[test]
fn group_by_can_reference_select_aliases() {
    for sql in [
        "SELECT user_id AS u, count(*) FROM orders GROUP BY u",
        "SELECT date_trunc('day', created_at) AS d, count(*) FROM users GROUP BY d ORDER BY d",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn default_keyword_is_not_a_column() {
    for sql in [
        "INSERT INTO orders (id, user_id, total) VALUES (DEFAULT, 1, 2)",
        "UPDATE users SET updated_at = DEFAULT WHERE id = 1",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn whole_row_references_are_allowed() {
    for sql in [
        "SELECT json_agg(u) FROM users u",
        "SELECT to_jsonb(o) FROM orders o WHERE o.id = 1",
        "SELECT row_to_json(users) FROM users",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn table_functions_without_alias_are_allowed() {
    for sql in [
        "SELECT * FROM generate_series(1, 10)",
        "SELECT generate_series FROM generate_series(1, 10)",
        "SELECT key, value FROM users, jsonb_each(metadata)",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
    assert_valid(
        SQLITE_SCHEMA,
        SqlDialect::SQLite,
        "SELECT value FROM json_each('[1, 2]')",
    );
}

#[test]
fn system_columns_and_tables_are_known() {
    for sql in [
        "DELETE FROM orders WHERE ctid IN (SELECT ctid FROM orders LIMIT 10)",
        "SELECT xmin, id FROM users",
        "SELECT tableoid::regclass, id FROM users",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
    for sql in [
        "SELECT rowid, name FROM users",
        "SELECT oid, _rowid_ FROM users",
        "SELECT name FROM sqlite_master WHERE type = 'table'",
        "SELECT name FROM sqlite_schema",
    ] {
        assert_valid(SQLITE_SCHEMA, SqlDialect::SQLite, sql);
    }
}

const MYSQL_SHOP: &str = r#"
    CREATE TABLE customers (id INT PRIMARY KEY, balance DECIMAL(10,2), created_at DATETIME);
    CREATE TABLE orders (id INT PRIMARY KEY, customer_id INT, total DECIMAL(10,2), placed_at DATETIME);
"#;

#[test]
fn mysql_multi_table_update_and_delete() {
    for sql in [
        "UPDATE orders o JOIN customers c ON c.id = o.customer_id SET c.balance = c.balance - o.total WHERE o.id = 1",
        "DELETE o FROM orders o JOIN customers c ON c.id = o.customer_id WHERE c.id = 1",
        "DELETE FROM o USING orders o JOIN customers c ON c.id = o.customer_id WHERE c.id = 1",
    ] {
        assert_valid(MYSQL_SHOP, SqlDialect::MySQL, sql);
    }
    let diagnostics = analyze(
        MYSQL_SHOP,
        SqlDialect::MySQL,
        "UPDATE orders o JOIN customers c ON c.id = o.customer_id SET c.balanse = 0",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn mysql_keywords_and_variables_are_not_columns() {
    for sql in [
        "SELECT TIMESTAMPDIFF(DAY, placed_at, NOW()) FROM orders",
        "SELECT id FROM orders WHERE customer_id = @uid",
        "SELECT DATE_ADD(placed_at, INTERVAL 1 DAY) FROM orders",
    ] {
        assert_valid(MYSQL_SHOP, SqlDialect::MySQL, sql);
    }
}

#[test]
fn insert_into_view_is_allowed() {
    let schema = format!("{PG_SCHEMA} CREATE VIEW active_users AS SELECT id, name FROM users;");
    assert_valid(
        &schema,
        SqlDialect::PostgreSQL,
        "INSERT INTO active_users (name) VALUES ('a')",
    );
}

#[test]
fn ambiguity_message_and_suggestions_are_deterministic() {
    let first = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users, orders",
    );
    for _ in 0..20 {
        let again = analyze(
            PG_SCHEMA,
            SqlDialect::PostgreSQL,
            "SELECT id FROM users, orders",
        );
        assert_eq!(first[0].message, again[0].message);
        assert_eq!(first[0].help, again[0].help);
    }
    assert_eq!(
        first[0].message,
        "Column 'id' is ambiguous (found in tables: users, orders)"
    );
}

#[test]
fn suggestions_require_real_similarity() {
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, "SELECT zz FROM users");
    assert_eq!(diagnostics[0].help, None, "{diagnostics:#?}");
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, "SELECT emial FROM users");
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("Did you mean 'email'?")
    );
}

// ---------------------------------------------------------------------------
// Missed errors: RETURNING, ON CONFLICT, other clauses
// ---------------------------------------------------------------------------

fn assert_single(schema: &str, dialect: SqlDialect, sql: &str, kind: DiagnosticKind) {
    let diagnostics = analyze(schema, dialect, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![kind],
        "for `{sql}`: {diagnostics:#?}"
    );
}

#[test]
fn returning_columns_are_checked() {
    for sql in [
        "INSERT INTO users (name) VALUES ('a') RETURNING nope",
        "UPDATE users SET name = 'x' WHERE id = 1 RETURNING nope",
        "DELETE FROM orders WHERE id = 1 RETURNING nope",
        "INSERT INTO orders AS o (user_id, total) VALUES (1, 2) RETURNING o.nope",
    ] {
        assert_single(
            PG_SCHEMA,
            SqlDialect::PostgreSQL,
            sql,
            DiagnosticKind::ColumnNotFound,
        );
    }
    for sql in [
        "INSERT INTO users (name) VALUES ('a') RETURNING id, users.name, *",
        "UPDATE orders o SET total = 0 FROM users u WHERE u.id = o.user_id RETURNING o.id, u.name",
        "DELETE FROM orders WHERE id = 1 RETURNING id, total * 2 AS doubled",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn on_conflict_and_on_duplicate_key_are_checked() {
    for sql in [
        "INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (nope) DO NOTHING",
        "INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET nope = 1",
        "INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.nope",
        "INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = 'b' WHERE users.nope > 1",
    ] {
        assert_single(PG_SCHEMA, SqlDialect::PostgreSQL, sql, DiagnosticKind::ColumnNotFound);
    }
    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (id, user_id, total) VALUES (1, 1, 2) ON CONFLICT (id) DO UPDATE SET total = 'lots'",
        DiagnosticKind::TypeMismatch,
    );
    assert_valid(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (id, user_id, total) VALUES (1, 1, 2) ON CONFLICT (id) DO UPDATE SET total = orders.total + EXCLUDED.total WHERE orders.user_id = 1",
    );
    assert_single(
        MYSQL_SHOP,
        SqlDialect::MySQL,
        "INSERT INTO customers (id, balance) VALUES (1, 2) ON DUPLICATE KEY UPDATE balanse = VALUES(balance)",
        DiagnosticKind::ColumnNotFound,
    );
    assert_valid(
        MYSQL_SHOP,
        SqlDialect::MySQL,
        "INSERT INTO customers (id, balance) VALUES (1, 2) ON DUPLICATE KEY UPDATE balance = balance + VALUES(balance)",
    );
    assert_single(
        SQLITE_SCHEMA,
        SqlDialect::SQLite,
        "INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = excluded.nme",
        DiagnosticKind::ColumnNotFound,
    );
}

#[test]
fn other_clauses_are_resolved() {
    for sql in [
        "SELECT DISTINCT ON (nope) id FROM users",
        "SELECT id, row_number() OVER w FROM users WINDOW w AS (PARTITION BY nope)",
        "SELECT array_agg(id ORDER BY nope) FROM users",
        "SELECT percentile_cont(0.5) WITHIN GROUP (ORDER BY nope) FROM orders",
        "SELECT id FROM users LIMIT (SELECT count(nope) FROM orders)",
    ] {
        assert_single(
            PG_SCHEMA,
            SqlDialect::PostgreSQL,
            sql,
            DiagnosticKind::ColumnNotFound,
        );
    }
}

#[test]
fn insert_select_column_count_is_checked() {
    for sql in [
        "INSERT INTO orders (user_id, total) SELECT id FROM users",
        "INSERT INTO orders (user_id) SELECT id, 1 FROM users",
    ] {
        assert_single(
            PG_SCHEMA,
            SqlDialect::PostgreSQL,
            sql,
            DiagnosticKind::ColumnCountMismatch,
        );
    }
    for sql in [
        "INSERT INTO orders (user_id, total) SELECT id, 0 FROM users",
        "INSERT INTO orders (user_id, total) SELECT * FROM (SELECT id, 0 FROM users) t",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn insert_values_expressions_are_type_checked() {
    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (user_id, total) VALUES (1, 1 + 'x')",
        DiagnosticKind::TypeMismatch,
    );
}

#[test]
fn using_column_must_exist_on_both_sides() {
    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT 1 FROM users JOIN orders USING (email)",
        DiagnosticKind::ColumnNotFound,
    );
}

#[test]
fn enum_literals_are_checked_against_enum_values() {
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users WHERE status = 'actve'",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
    assert_eq!(
        diagnostics[0].help.as_deref(),
        Some("Did you mean 'active'?")
    );
    for sql in [
        "UPDATE users SET status = 'deleted' WHERE id = 1",
        "INSERT INTO users (name, status) VALUES ('a', 'pending')",
    ] {
        assert_single(
            PG_SCHEMA,
            SqlDialect::PostgreSQL,
            sql,
            DiagnosticKind::TypeMismatch,
        );
    }
}

// ---------------------------------------------------------------------------
// Spans for literals and parse errors
// ---------------------------------------------------------------------------

#[test]
fn literal_type_mismatches_have_locations() {
    let sql = "SELECT 1;\nUPDATE orders SET placed_on = 5 WHERE id = 1;";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
    assert_eq!(span_text(sql, &diagnostics[0]), "placed_on");

    let sql = "INSERT INTO orders (user_id, total)\nVALUES (1, 'free');";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
    assert_eq!(span_text(sql, &diagnostics[0]), "total");

    let sql = "SELECT id FROM users WHERE 'abc' = id";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
    assert_eq!(span_text(sql, &diagnostics[0]), "id");
}

#[test]
fn parse_error_has_location_and_does_not_hide_other_statements() {
    let sql = "SELECT naem FROM users;\nSELECT id,\n  FROM users WHERE;\nSELECT emial FROM users;";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![
            DiagnosticKind::ColumnNotFound,
            DiagnosticKind::ParseError,
            DiagnosticKind::ColumnNotFound
        ],
        "{diagnostics:#?}"
    );
    let parse_error = &diagnostics[1];
    let span = parse_error.span.expect("parse error should have a span");
    assert_eq!(span.line, 3, "{parse_error:#?}");
    assert!(
        !parse_error.message.contains("sql parser error")
            && !parse_error.message.contains("at Line:"),
        "message should be clean: {}",
        parse_error.message
    );
    // spans of later statements are still absolute
    assert_eq!(diagnostics[2].span.unwrap().line, 4);
}

#[test]
fn parse_error_at_end_of_input_points_at_the_end() {
    let sql = "SELECT id FROM users WHERE";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ParseError]);
    let span = diagnostics[0].span.unwrap();
    assert_eq!(span.line, 1);
    assert!(span.column >= 21, "{span:?}");
}

#[test]
fn parse_errors_can_be_suppressed_inline() {
    let sql = "SELECT id FROM users WHERE; -- sqlsift:disable E1000";
    assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
}

#[test]
fn null_into_auto_generated_integer_key() {
    // MySQL AUTO_INCREMENT / SQLite rowid alias generate the key from NULL
    assert_valid(
        "CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY, name TEXT);",
        SqlDialect::MySQL,
        "INSERT INTO t (id, name) VALUES (NULL, 'a')",
    );
    assert_valid(
        SQLITE_SCHEMA,
        SqlDialect::SQLite,
        "INSERT INTO users (id, name) VALUES (NULL, 'a')",
    );
    // PostgreSQL rejects NULL for serial / identity columns
    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (id, user_id, total) VALUES (NULL, 1, 2)",
        DiagnosticKind::PotentialNullViolation,
    );
}

// ---------------------------------------------------------------------------
// CASE branch types and MySQL inline ENUM values
// ---------------------------------------------------------------------------

#[test]
fn case_branches_with_incompatible_types_are_reported() {
    for sql in [
        "SELECT CASE WHEN total > 10 THEN total ELSE 'cheap' END FROM orders",
        "SELECT CASE WHEN is_admin THEN 1 WHEN id > 5 THEN created_at END FROM users",
        "SELECT id FROM users WHERE CASE WHEN is_admin THEN id ELSE name END = 1",
    ] {
        let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
        assert!(
            kinds(&diagnostics).contains(&DiagnosticKind::TypeMismatch),
            "for `{sql}`: {diagnostics:#?}"
        );
        assert!(
            diagnostics.iter().any(|d| d.message.contains("CASE")),
            "for `{sql}`: {diagnostics:#?}"
        );
    }
}

#[test]
fn case_branches_with_compatible_types_are_fine() {
    for sql in [
        "SELECT CASE WHEN total > 10 THEN 'big' ELSE 'small' END FROM orders",
        "SELECT CASE WHEN total > 10 THEN total ELSE 0 END FROM orders",
        "SELECT CASE WHEN total > 10 THEN total ELSE '0.5' END FROM orders",
        "SELECT CASE status WHEN 'active' THEN 1 ELSE 0 END FROM users",
        "SELECT CASE WHEN is_admin THEN created_at ELSE NULL END FROM users",
        "SELECT CASE WHEN is_admin THEN name ELSE email END FROM users",
        "SELECT id FROM users WHERE CASE WHEN is_admin THEN 1 ELSE 2 END = 1",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
}

#[test]
fn case_result_type_is_inferred() {
    // CASE ... THEN integer: comparing the result with text is a mismatch
    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "SELECT id FROM users WHERE CASE WHEN is_admin THEN 1 ELSE 2 END = 'x'",
        DiagnosticKind::TypeMismatch,
    );
}

const MYSQL_ENUM: &str = r#"
    CREATE TABLE tickets (
        id INT AUTO_INCREMENT PRIMARY KEY,
        status ENUM('open', 'closed', 'pending') NOT NULL DEFAULT 'open',
        priority INT
    );
"#;

#[test]
fn mysql_inline_enum_values_are_checked() {
    let diagnostics = analyze(
        MYSQL_ENUM,
        SqlDialect::MySQL,
        "SELECT id FROM tickets WHERE status = 'opne'",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TypeMismatch]);
    assert_eq!(diagnostics[0].help.as_deref(), Some("Did you mean 'open'?"));
    for sql in [
        "INSERT INTO tickets (status) VALUES ('archived')",
        "UPDATE tickets SET status = 'done' WHERE id = 1",
    ] {
        assert_single(
            MYSQL_ENUM,
            SqlDialect::MySQL,
            sql,
            DiagnosticKind::TypeMismatch,
        );
    }
    for sql in [
        "SELECT id FROM tickets WHERE status = 'pending'",
        "INSERT INTO tickets (status) VALUES ('closed')",
        "SELECT id FROM tickets WHERE status IN ('open', 'closed') ORDER BY status",
        "SELECT id FROM tickets WHERE status = 1",
    ] {
        assert_valid(MYSQL_ENUM, SqlDialect::MySQL, sql);
    }
}

// ---------------------------------------------------------------------------
// E0008: INSERT omits a NOT NULL column without a default
// ---------------------------------------------------------------------------

#[test]
fn insert_missing_required_column_is_reported() {
    let sql = "INSERT INTO orders (user_id) VALUES (1)";
    let diagnostics = analyze(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::MissingRequiredColumn]
    );
    assert_eq!(diagnostics[0].code(), "E0008");
    assert!(
        diagnostics[0].message.contains("'total'"),
        "{diagnostics:#?}"
    );
    assert_eq!(span_text(sql, &diagnostics[0]), "orders");

    // several missing columns are reported together
    let diagnostics = analyze(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (placed_on) SELECT current_date",
    );
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::MissingRequiredColumn]
    );
    assert!(
        diagnostics[0].message.contains("'user_id'") && diagnostics[0].message.contains("'total'"),
        "{diagnostics:#?}"
    );

    assert_single(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO users DEFAULT VALUES",
        DiagnosticKind::MissingRequiredColumn,
    );
}

#[test]
fn columns_with_generated_values_are_not_required() {
    // serial / identity / DEFAULT / nullable columns may be omitted
    for sql in [
        "INSERT INTO orders (user_id, total) VALUES (1, 2)",
        "INSERT INTO users (name) VALUES ('a')",
        "INSERT INTO orders (user_id, total) SELECT id, 0 FROM users",
    ] {
        assert_valid(PG_SCHEMA, SqlDialect::PostgreSQL, sql);
    }
    assert_valid(
        "CREATE TABLE t (id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, n INT NOT NULL DEFAULT 0, label TEXT);",
        SqlDialect::PostgreSQL,
        "INSERT INTO t (label) VALUES ('x')",
    );
    // MySQL AUTO_INCREMENT
    assert_valid(
        "CREATE TABLE t (id INT NOT NULL AUTO_INCREMENT, name VARCHAR(10) NOT NULL, PRIMARY KEY (id));",
        SqlDialect::MySQL,
        "INSERT INTO t (name) VALUES ('a')",
    );
    // SQLite rowid alias
    assert_valid(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        SqlDialect::SQLite,
        "INSERT INTO t (name) VALUES ('a')",
    );
    // ... but a composite integer key is not generated
    assert_single(
        "CREATE TABLE t (a INTEGER NOT NULL, b INTEGER NOT NULL, PRIMARY KEY (a, b));",
        SqlDialect::SQLite,
        "INSERT INTO t (a) VALUES (1)",
        DiagnosticKind::MissingRequiredColumn,
    );
}

#[test]
fn missing_required_column_can_be_disabled_inline() {
    assert_valid(
        PG_SCHEMA,
        SqlDialect::PostgreSQL,
        "INSERT INTO orders (user_id) VALUES (1); -- sqlsift:disable E0008",
    );
}
