//! Structural rules: duplicate names (E0009), duplicate target columns (E0010),
//! positions out of range (E0011) and misplaced aggregates (E0012).
//!
//! Every query reported here is rejected by the database (checked against
//! PostgreSQL 16 and SQLite 3.45); every query asserted clean is accepted.

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::{Catalog, SchemaBuilder};
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT, org_id INT);
CREATE TABLE orgs (id INT PRIMARY KEY, title TEXT);
CREATE SCHEMA auth;
CREATE TABLE auth.users (id INT PRIMARY KEY);
";

fn catalog(dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(SCHEMA).expect("schema parses");
    builder.build().0
}

fn codes_in(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    let catalog = catalog(dialect);
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
    let found = codes(sql);
    assert!(
        found.contains(&code),
        "expected {code} for {sql}, got {found:?}"
    );
}

#[track_caller]
fn assert_clean(sql: &str) {
    assert_eq!(codes(sql), Vec::<&str>::new(), "{sql}");
}

// ---------------------------------------------------------------------
// E0009 duplicate-name
// ---------------------------------------------------------------------

#[test]
fn e0009_alias_given_twice() {
    assert_reports("SELECT u.id FROM users u JOIN orgs u ON true", "E0009");
    assert_reports("SELECT 1 FROM users, users", "E0009");
    assert_reports("SELECT 1 FROM users u, (SELECT 1) u", "E0009");
    assert_reports(
        "SELECT 1 FROM users JOIN (orgs JOIN users ON true) ON true",
        "E0009",
    );
}

#[test]
fn e0009_cte_name_given_twice() {
    assert_reports(
        "WITH x AS (SELECT 1), x AS (SELECT 2) SELECT * FROM x",
        "E0009",
    );
}

#[test]
fn e0009_distinct_names_are_fine() {
    assert_clean("SELECT a.id, b.id FROM users a JOIN users b ON a.id = b.org_id");
    // Two tables both named `users`, in different schemas
    assert_clean("SELECT 1 FROM auth.users, public.users");
    // The same name in different query blocks
    assert_clean("SELECT u.id FROM users u WHERE EXISTS (SELECT 1 FROM orgs u WHERE u.id = 1)");
    assert_clean(
        "WITH x AS (SELECT 1 AS a) SELECT * FROM x, (WITH x AS (SELECT 2 AS b) SELECT * FROM x) y",
    );
}

#[test]
fn e0009_dialects() {
    // SQLite accepts a table alias given twice, but not a CTE name
    assert!(codes_in(SqlDialect::SQLite, "SELECT 1 FROM users, users").is_empty());
    assert_eq!(
        codes_in(
            SqlDialect::SQLite,
            "WITH x AS (SELECT 1), x AS (SELECT 2) SELECT * FROM x"
        ),
        vec!["E0009"]
    );
    // MySQL aliases are case sensitive
    assert!(codes_in(SqlDialect::MySQL, "SELECT 1 FROM users U, orgs u").is_empty());
    assert_eq!(
        codes_in(SqlDialect::MySQL, "SELECT 1 FROM users u, orgs u"),
        vec!["E0009"]
    );
}

// ---------------------------------------------------------------------
// E0010 duplicate-target-column
// ---------------------------------------------------------------------

#[test]
fn e0010_insert_column_given_twice() {
    assert_eq!(
        codes("INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"),
        vec!["E0010"]
    );
    assert_eq!(
        codes_in(
            SqlDialect::MySQL,
            "INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"
        ),
        vec!["E0010"]
    );
    // SQLite accepts it
    assert!(codes_in(
        SqlDialect::SQLite,
        "INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"
    )
    .is_empty());
}

#[test]
fn e0010_update_column_assigned_twice() {
    assert_eq!(
        codes("UPDATE users SET name = 'a', name = 'b'"),
        vec!["E0010"]
    );
    assert_eq!(
        codes(
            "INSERT INTO users (id, name) VALUES (1, 'a') \
             ON CONFLICT (id) DO UPDATE SET name = 'b', name = 'c'"
        ),
        vec!["E0010"]
    );
    // MySQL and SQLite apply the assignments in order
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        assert!(codes_in(dialect, "UPDATE users SET name = 'a', name = 'b'").is_empty());
    }
    assert_clean("UPDATE users SET name = 'a', email = 'b'");
}

// ---------------------------------------------------------------------
// E0011 position-out-of-range
// ---------------------------------------------------------------------

#[test]
fn e0011_order_and_group_by_positions() {
    assert_eq!(codes("SELECT id FROM users ORDER BY 2"), vec!["E0011"]);
    assert_eq!(codes("SELECT id FROM users ORDER BY 0"), vec!["E0011"]);
    assert_eq!(codes("SELECT id FROM users GROUP BY 3"), vec!["E0011"]);
    assert_eq!(
        codes("SELECT id FROM users UNION SELECT id FROM orgs ORDER BY 2"),
        vec!["E0011"]
    );
    assert_eq!(
        codes_in(SqlDialect::SQLite, "SELECT id FROM users ORDER BY 2"),
        vec!["E0011"]
    );
}

#[test]
fn e0011_positions_in_range_are_fine() {
    assert_clean("SELECT id, name FROM users ORDER BY 2, 1");
    assert_clean("SELECT * FROM users ORDER BY 4");
    assert_clean("SELECT org_id, count(*) FROM users GROUP BY 1 ORDER BY 2 DESC");
    assert_clean("SELECT id FROM users UNION SELECT id FROM orgs ORDER BY 1");
    // Constant expressions are not positions
    assert_clean("SELECT id FROM users ORDER BY 2 + 0");
    // Unknown columns: no count to compare with
    assert_clean("SELECT * FROM generate_series(1, 3) ORDER BY 2");
}

// ---------------------------------------------------------------------
// E0012 misplaced-aggregate
// ---------------------------------------------------------------------

#[test]
fn e0012_aggregates_where_not_allowed() {
    assert_eq!(
        codes("SELECT id FROM users WHERE count(*) > 1"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT 1 FROM users GROUP BY count(*)"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT 1 FROM users u JOIN orgs o ON max(o.id) = u.org_id"),
        vec!["E0012"]
    );
    assert_eq!(codes("UPDATE users SET org_id = max(id)"), vec!["E0012"]);
    assert_eq!(codes("DELETE FROM users WHERE sum(id) > 1"), vec!["E0012"]);
    assert_eq!(
        codes("INSERT INTO users (id, name) VALUES (count(*), 'a')"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_nested_aggregates() {
    assert_eq!(codes("SELECT max(count(*)) FROM users"), vec!["E0012"]);
    assert_eq!(
        codes("SELECT org_id FROM users GROUP BY org_id HAVING sum(count(*)) > 1"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_window_functions_where_not_allowed() {
    assert_eq!(
        codes("SELECT id FROM users WHERE row_number() OVER () > 1"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT org_id FROM users GROUP BY org_id HAVING rank() OVER () = 1"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_allowed_places_are_fine() {
    assert_clean("SELECT org_id, count(*) FROM users GROUP BY org_id HAVING count(*) > 1");
    assert_clean("SELECT org_id FROM users GROUP BY org_id ORDER BY count(*)");
    // Aggregates of a subquery belong to it
    assert_clean("SELECT id FROM users WHERE id > (SELECT max(id) FROM orgs)");
    assert_clean("SELECT id FROM users WHERE EXISTS (SELECT 1 FROM orgs HAVING count(*) > 0)");
    assert_clean("UPDATE users SET org_id = (SELECT max(id) FROM orgs)");
    // A window function over an aggregate
    assert_clean("SELECT org_id, sum(count(*)) OVER () FROM users GROUP BY org_id");
    assert_clean("SELECT id, row_number() OVER (ORDER BY id) FROM users");
    // A user-defined function named like an aggregate, schema-qualified
    assert_clean("SELECT id FROM users WHERE myschema.max(id) > 1");
}

#[test]
fn e0012_sqlite_scalar_min_max() {
    // With several arguments SQLite's min/max are scalar functions
    assert!(codes_in(
        SqlDialect::SQLite,
        "SELECT id FROM users WHERE max(id, 2) > 1"
    )
    .is_empty());
    assert_eq!(
        codes_in(SqlDialect::SQLite, "SELECT id FROM users WHERE max(id) > 1"),
        vec!["E0012"]
    );
}
