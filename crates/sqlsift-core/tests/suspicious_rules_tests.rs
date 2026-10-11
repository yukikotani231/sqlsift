//! Suspicious rules (warnings by default): comparisons with NULL (E0018), NOT IN
//! over a nullable column (E0019), an IN subquery selecting an outer column
//! (E0020) and FROM tables no condition links (E0021).

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::Severity;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT, org_id INT, tags INT[]);
CREATE TABLE orgs (oid INT PRIMARY KEY, title TEXT, owner_id INT NOT NULL);
CREATE TABLE posts (id INT PRIMARY KEY, user_id INT);
";

fn codes_in(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

fn codes(sql: &str) -> Vec<&'static str> {
    codes_in(SqlDialect::PostgreSQL, sql)
}

#[track_caller]
fn assert_reports(sql: &str, code: &str) {
    assert_eq!(codes(sql), [code], "{sql}");
}

#[track_caller]
fn assert_clean(sql: &str) {
    assert_eq!(codes(sql), Vec::<&str>::new(), "{sql}");
}

#[test]
fn suspicious_rules_are_warnings() {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).unwrap();
    let catalog = builder.build().0;
    let diagnostics = Analyzer::new(&catalog).analyze("SELECT id FROM users WHERE email = NULL");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Severity::Warning);
}

// ---------------------------------------------------------------------
// E0018 null-comparison
// ---------------------------------------------------------------------

#[test]
fn e0018_comparison_with_null() {
    assert_reports("SELECT id FROM users WHERE email = NULL", "E0018");
    assert_reports("SELECT id FROM users WHERE email <> NULL", "E0018");
    assert_reports("SELECT id FROM users WHERE NULL != email", "E0018");
    assert_reports("SELECT id FROM users WHERE org_id > (NULL)", "E0018");
    assert_reports("SELECT id FROM users WHERE id = 1 OR email = NULL", "E0018");
    assert_reports("UPDATE users SET name = 'x' WHERE email = NULL", "E0018");
    assert_reports(
        "SELECT CASE email WHEN NULL THEN 'none' ELSE email END FROM users",
        "E0018",
    );
}

#[test]
fn e0018_null_tests_and_assignments_are_fine() {
    assert_clean("SELECT id FROM users WHERE email IS NULL");
    assert_clean("SELECT id FROM users WHERE email IS NOT DISTINCT FROM NULL");
    assert_clean("UPDATE users SET email = NULL WHERE id = 1");
    assert_clean("SELECT CASE WHEN email IS NULL THEN 'none' END FROM users");
    assert_clean("SELECT coalesce(email, NULL) FROM users");
    // MySQL's NULL-safe equality
    assert!(codes_in(
        SqlDialect::MySQL,
        "SELECT id FROM users WHERE email <=> NULL"
    )
    .is_empty());
}

// ---------------------------------------------------------------------
// E0019 not-in-with-nulls
// ---------------------------------------------------------------------

#[test]
fn e0019_not_in_over_nullable_column() {
    assert_reports(
        "SELECT id FROM users WHERE id NOT IN (SELECT user_id FROM posts)",
        "E0019",
    );
    assert_reports(
        "SELECT id FROM users u WHERE u.id NOT IN (SELECT p.user_id FROM posts p WHERE p.id > 10)",
        "E0019",
    );
}

#[test]
fn e0019_columns_without_nulls_are_fine() {
    // NOT NULL, primary key, NULLs filtered out
    assert_clean("SELECT id FROM users WHERE id NOT IN (SELECT owner_id FROM orgs)");
    assert_clean("SELECT id FROM users WHERE org_id NOT IN (SELECT oid FROM orgs)");
    assert_clean(
        "SELECT id FROM users WHERE id NOT IN (SELECT user_id FROM posts WHERE user_id IS NOT NULL)",
    );
    assert_clean(
        "SELECT id FROM users WHERE id NOT IN (SELECT user_id FROM posts WHERE user_id > 0)",
    );
    assert_clean(
        "SELECT id FROM users WHERE id NOT IN (SELECT p.user_id FROM posts p JOIN users u ON u.id = p.user_id)",
    );
    // IN is fine; so is NOT EXISTS
    assert_clean("SELECT id FROM users WHERE id IN (SELECT user_id FROM posts)");
    assert_clean(
        "SELECT id FROM users u WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.user_id = u.id)",
    );
    // A derived table or CTE: nullability unknown
    assert_clean(
        "WITH p AS (SELECT user_id FROM posts) SELECT id FROM users WHERE id NOT IN (SELECT user_id FROM p)",
    );
}

// ---------------------------------------------------------------------
// E0020 outer-column-in-subquery
// ---------------------------------------------------------------------

#[test]
fn e0020_in_subquery_selects_outer_column() {
    assert_reports(
        "SELECT id FROM users WHERE id IN (SELECT id FROM orgs)",
        "E0020",
    );
    assert_reports(
        "SELECT id FROM users WHERE name NOT IN (SELECT name FROM orgs)",
        "E0020",
    );
    assert_reports(
        "DELETE FROM users WHERE org_id IN (SELECT org_id FROM orgs WHERE title = 'x')",
        "E0020",
    );
}

#[test]
fn e0020_subquery_columns_are_fine() {
    assert_clean("SELECT id FROM users WHERE org_id IN (SELECT oid FROM orgs)");
    assert_clean("SELECT id FROM users WHERE id IN (SELECT user_id FROM posts)");
    // Qualified: clearly meant
    assert_clean("SELECT id FROM users u WHERE u.id IN (SELECT u.id FROM orgs)");
    // A column of the subquery's table shadows the outer one
    assert_clean("SELECT id FROM users WHERE id IN (SELECT id FROM posts)");
    // Correlated references elsewhere in the subquery are normal
    assert_clean("SELECT id FROM users WHERE org_id IN (SELECT oid FROM orgs WHERE owner_id = id)");
    // Columns of the subquery's relation unknown
    assert_clean("SELECT id FROM users WHERE id IN (SELECT id FROM generate_series(1, 3) g)");
}

// ---------------------------------------------------------------------
// E0021 missing-join-condition
// ---------------------------------------------------------------------

#[test]
fn e0021_tables_without_join_condition() {
    assert_reports("SELECT u.id FROM users u, orgs o", "E0021");
    assert_reports(
        "SELECT u.id FROM users u, orgs o, posts p WHERE u.org_id = o.oid",
        "E0021",
    );
}

#[test]
fn e0021_linked_or_deliberate_cross_joins_are_fine() {
    assert_clean("SELECT u.id FROM users u, orgs o WHERE u.org_id = o.oid");
    assert_clean("SELECT u.id FROM users u CROSS JOIN orgs o");
    assert_clean("SELECT u.id FROM users u JOIN orgs o ON true");
    // Linked through another table, or a subquery
    assert_clean(
        "SELECT u.id FROM users u, orgs o, posts p WHERE u.org_id = o.oid AND p.user_id = u.id",
    );
    assert_clean(
        "SELECT u.id FROM users u, orgs o WHERE EXISTS (SELECT 1 FROM posts p WHERE p.user_id = u.id AND p.id = o.oid)",
    );
    // A filter on one side: every row combined with the chosen ones
    assert_clean("SELECT u.id, o.oid FROM users u, orgs o WHERE o.title = 'admins'");
    // CTEs, subqueries and functions may return a single row
    assert_clean("WITH p AS (SELECT 1 AS n) SELECT u.id FROM users u, p");
    assert_clean("SELECT u.id FROM users u, (SELECT max(oid) AS m FROM orgs) m");
    assert_clean("SELECT u.id, g FROM users u, generate_series(1, 3) g");
    // Implicitly lateral: unnest of a column of an earlier item
    assert_clean("SELECT u.id, t FROM users u, unnest(u.tags) t");
    assert_clean(
        "SELECT u.id FROM users u, LATERAL (SELECT oid FROM orgs WHERE owner_id = u.id) o",
    );
    // A misspelled column may be the link
    assert_eq!(
        codes("SELECT u.id FROM users u, orgs o WHERE org = oid"),
        ["E0002"]
    );
}

#[test]
fn e0021_bigquery_array_paths_are_linked() {
    let bq = |sql| codes_in(SqlDialect::BigQuery, sql);
    assert!(bq("SELECT u.id FROM users u, u.tags t").is_empty());
    assert!(bq("SELECT u.id FROM users u, UNNEST(u.tags) t").is_empty());
}
