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
    assert_eq!(
        kinds(&diagnostics),
        vec![
            DiagnosticKind::TableNotFound,
            DiagnosticKind::ColumnNotFound
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
