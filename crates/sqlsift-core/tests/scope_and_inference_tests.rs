// Tests for the unified scope model: name resolution and type inference share one
// view of which relations are visible, and column types flow through CTEs,
// subqueries, views and CREATE TABLE AS.
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::SqlType;

const SCHEMA: &str = r"
    CREATE TYPE order_status AS ENUM ('pending', 'paid', 'shipped');
    CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        email TEXT,
        created_at TIMESTAMP
    );
    CREATE TABLE orders (
        id INTEGER PRIMARY KEY,
        user_id INTEGER NOT NULL REFERENCES users(id),
        total NUMERIC(10, 2),
        status order_status NOT NULL DEFAULT 'pending'
    );
    CREATE VIEW user_names AS SELECT id, name FROM users;
    CREATE VIEW big_spenders AS
        SELECT u.id AS user_id, SUM(o.total) AS spent
        FROM users u JOIN orders o ON o.user_id = u.id
        GROUP BY u.id;
    CREATE TABLE user_copy AS SELECT id, name FROM users;
";

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::PostgreSQL);
    builder.parse(SCHEMA).unwrap();
    builder.build().0
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    let catalog = catalog();
    Analyzer::with_dialect(&catalog, SqlDialect::PostgreSQL).analyze(sql)
}

fn assert_valid(sql: &str) {
    let diagnostics = analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics for `{sql}`, got: {diagnostics:#?}"
    );
}

/// Assert exactly one diagnostic of `kind` whose message contains `message`
fn assert_single(sql: &str, kind: DiagnosticKind, message: &str) {
    let diagnostics = analyze(sql);
    assert_eq!(diagnostics.len(), 1, "for `{sql}`: {diagnostics:#?}");
    assert_eq!(diagnostics[0].kind, kind, "for `{sql}`: {diagnostics:#?}");
    assert!(
        diagnostics[0].message.contains(message),
        "for `{sql}`: expected message containing {message:?}, got {diagnostics:#?}"
    );
}

// ---------------------------------------------------------------------------
// Column types flow through CTEs, subqueries and views
// ---------------------------------------------------------------------------

#[test]
fn cte_column_has_the_type_of_its_expression() {
    assert_single(
        "WITH t AS (SELECT id FROM users) SELECT * FROM t WHERE id = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
    assert_single(
        "WITH t AS (SELECT id FROM users) SELECT * FROM t WHERE t.id = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
}

#[test]
fn cte_explicit_column_list_keeps_types() {
    assert_single(
        "WITH t(user_key) AS (SELECT id FROM users) SELECT * FROM t WHERE user_key = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
}

#[test]
fn derived_table_column_has_the_type_of_its_expression() {
    assert_single(
        "SELECT * FROM (SELECT name FROM users) s WHERE s.name + 1 > 0",
        DiagnosticKind::TypeMismatch,
        "Arithmetic operation requires numeric types, but got text",
    );
}

#[test]
fn view_columns_have_inferred_types() {
    assert_single(
        "SELECT * FROM user_names WHERE id = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
    // Aggregates in the view: SUM(numeric) stays numeric
    assert_valid("SELECT * FROM big_spenders WHERE spent > 100.5 AND user_id = 3");
    assert_single(
        "SELECT * FROM big_spenders WHERE spent = 'lots'",
        DiagnosticKind::TypeMismatch,
        "cannot compare numeric",
    );
}

#[test]
fn view_column_types_are_stored_in_the_catalog() {
    let catalog = catalog();
    let view = catalog
        .get_view(&QualifiedName::new("user_names"))
        .expect("view");
    assert_eq!(view.columns, ["id", "name"]);
    assert_eq!(view.column_types, [SqlType::Integer, SqlType::Text]);
}

#[test]
fn create_table_as_columns_have_inferred_types() {
    let catalog = catalog();
    let table = catalog
        .get_table(&QualifiedName::new("user_copy"))
        .expect("table");
    assert_eq!(table.get_column("id").unwrap().data_type, SqlType::Integer);
    assert_eq!(table.get_column("name").unwrap().data_type, SqlType::Text);
    assert_single(
        "SELECT * FROM user_copy WHERE id = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
}

#[test]
fn scalar_subquery_has_the_type_of_its_column() {
    assert_single(
        "SELECT * FROM users WHERE id = (SELECT name FROM users LIMIT 1)",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
    assert_valid("SELECT * FROM users WHERE id = (SELECT MAX(user_id) FROM orders)");
}

#[test]
fn in_subquery_value_is_compared_with_the_subquery_column() {
    assert_single(
        "SELECT * FROM users WHERE id IN (SELECT name FROM users)",
        DiagnosticKind::TypeMismatch,
        "column 'name' of the subquery",
    );
    assert_valid("SELECT * FROM users WHERE id IN (SELECT user_id FROM orders)");
    // Several columns or an unexpandable wildcard: nothing to compare
    assert_valid("SELECT * FROM users WHERE id IN (SELECT * FROM generate_series(1, 3))");
}

#[test]
fn recursive_cte_columns_come_from_the_anchor() {
    assert_single(
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 10)
         SELECT n FROM r WHERE n = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
    // The recursive term sees the anchor's column names
    assert_single(
        "WITH RECURSIVE r AS (SELECT 1 AS n UNION ALL SELECT m + 1 FROM r) SELECT n FROM r",
        DiagnosticKind::ColumnNotFound,
        "'m'",
    );
}

#[test]
fn returning_columns_of_a_data_modifying_cte_have_types() {
    assert_valid(
        "WITH moved AS (UPDATE orders SET total = 0 WHERE id = 1 RETURNING id, total)
         SELECT * FROM moved WHERE total > 5",
    );
    assert_single(
        "WITH moved AS (UPDATE orders SET total = 0 WHERE id = 1 RETURNING id, total)
         SELECT * FROM moved WHERE total = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare numeric",
    );
    assert_single(
        "WITH added AS (INSERT INTO orders (id, user_id) VALUES (1, 2) RETURNING *)
         SELECT status FROM added WHERE user_id = 'abc'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
}

#[test]
fn set_operation_column_type_comes_from_the_first_known_branch() {
    // NULL on the left: the column is an integer
    assert_valid(
        "WITH u AS (SELECT NULL AS v UNION ALL SELECT id FROM users) SELECT * FROM u WHERE v = 1",
    );
    assert_single(
        "WITH u AS (SELECT NULL AS v UNION ALL SELECT id FROM users) SELECT * FROM u WHERE v = 'x'",
        DiagnosticKind::TypeMismatch,
        "cannot compare integer with text",
    );
}

#[test]
fn enum_values_are_checked_in_join_conditions() {
    assert_single(
        "SELECT * FROM users u JOIN orders o ON o.user_id = u.id AND o.status = 'completed'",
        DiagnosticKind::TypeMismatch,
        "Invalid value 'completed' for enum type 'order_status'",
    );
    assert_valid("SELECT * FROM users u JOIN orders o ON o.user_id = u.id AND o.status = 'paid'");
}

#[test]
fn inferred_types_do_not_cause_false_positives() {
    for sql in [
        // aggregates, MAX over enums/text, date comparison in a nested subquery
        "WITH s AS (SELECT user_id, SUM(total) AS spent, MAX(o.status) AS st FROM orders o GROUP BY user_id)
         SELECT * FROM s WHERE spent > 100 AND st = 'paid'
           AND user_id IN (SELECT id FROM users WHERE created_at > '2024-01-01')",
        "SELECT * FROM (SELECT user_id, COUNT(*) AS c FROM orders GROUP BY user_id) t WHERE t.c >= 2",
        // GROUP BY / ORDER BY output aliases inside a derived table
        "SELECT * FROM (SELECT user_id AS uid, COUNT(*) AS n FROM orders GROUP BY uid ORDER BY n DESC) t
         WHERE t.n > 1",
        // CASE producing text
        "SELECT * FROM (SELECT CASE WHEN id > 1 THEN 'big' ELSE 'small' END AS size FROM users) t
         WHERE size = 'big'",
        // qualified wildcard in a CTE
        "WITH t AS (SELECT u.*, o.total FROM users u JOIN orders o ON o.user_id = u.id)
         SELECT name, total FROM t WHERE total > 1 AND name LIKE 'a%'",
        // VALUES as a relation
        "SELECT * FROM (VALUES (1, 'a'), (2, 'b')) AS v(n, label) WHERE n = 1 AND label = 'a'",
        // column types of an unknown function stay unknown
        "WITH t AS (SELECT some_function(id) AS x FROM users) SELECT * FROM t WHERE x = 'abc' AND x = 1",
    ] {
        assert_valid(sql);
    }
}

// ---------------------------------------------------------------------------
// Visibility rules
// ---------------------------------------------------------------------------

#[test]
fn nearest_enclosing_query_block_wins() {
    // `id` is in both users u and orders o; the nearer block (o) provides it
    assert_valid(
        "SELECT * FROM users u WHERE EXISTS (
           SELECT 1 FROM orders o WHERE EXISTS (
             SELECT 1 FROM (SELECT 1 AS z) x WHERE z = id))",
    );
    // ... but two relations of the same block are still ambiguous
    assert_single(
        "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o, users x WHERE id = 1)",
        DiagnosticKind::AmbiguousColumn,
        "'id' is ambiguous",
    );
}

#[test]
fn cte_is_not_visible_outside_its_query() {
    assert_single(
        "SELECT (WITH x AS (SELECT 1 AS a) SELECT a FROM x), (SELECT a FROM x) FROM users",
        DiagnosticKind::TableNotFound,
        "Table 'x' not found",
    );
}

#[test]
fn cte_is_visible_in_nested_subqueries() {
    assert_valid(
        "WITH t AS (SELECT id FROM users)
         SELECT * FROM orders WHERE user_id IN (SELECT id FROM (SELECT id FROM t) s)",
    );
}

#[test]
fn non_lateral_derived_table_cannot_see_its_from_clause() {
    assert_single(
        "SELECT * FROM users u, (SELECT total FROM orders o WHERE o.user_id = u.id) t",
        DiagnosticKind::TableNotFound,
        "'u'",
    );
    // LATERAL can, and its columns are typed
    assert_valid(
        "SELECT * FROM users u, LATERAL (SELECT total FROM orders o WHERE o.user_id = u.id) t
         WHERE t.total > 10",
    );
}

#[test]
fn derived_table_in_a_subquery_can_reference_the_outer_query() {
    assert_valid(
        "SELECT (SELECT c FROM (SELECT COUNT(*) AS c FROM orders o WHERE o.user_id = u.id) t)
         FROM users u",
    );
}

#[test]
fn parenthesized_join_registers_its_tables() {
    assert_valid("SELECT a.id, b.total FROM (users a JOIN orders b ON a.id = b.user_id)");
    // With an alias, the joined columns are reachable through it
    assert_valid(
        "SELECT j.name, j.total FROM (users a JOIN orders b ON a.id = b.user_id) AS j
         WHERE j.total > 1",
    );
}

#[test]
fn unaliased_derived_table_columns_are_visible() {
    assert_valid("SELECT x FROM (SELECT 1 AS x)");
}

#[test]
fn using_join_columns_appear_once_in_star() {
    assert_valid("WITH j AS (SELECT * FROM users JOIN orders USING (id)) SELECT id, total FROM j");
}

#[test]
fn set_operation_order_by_sees_output_columns() {
    assert_valid("SELECT id FROM users UNION SELECT user_id FROM orders ORDER BY id");
    assert_single(
        "SELECT id FROM users UNION SELECT user_id FROM orders ORDER BY nope",
        DiagnosticKind::ColumnNotFound,
        "'nope'",
    );
}
