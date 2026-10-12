//! Restriction rules (off by default): UPDATE/DELETE without WHERE (E0024),
//! INSERT without a column list (E0025), SELECT * (E0026) and LIMIT / OFFSET
//! without ORDER BY (E0028).

use sqlsift_core::rules::{RuleConfig, RuleLevel};
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT);
CREATE TABLE archived (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT);
";

fn diagnostics_with(rules: RuleConfig, sql: &str) -> Vec<sqlsift_core::Diagnostic> {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::new(&catalog).with_rules(rules).analyze(sql)
}

fn codes_with(rules: RuleConfig, sql: &str) -> Vec<&'static str> {
    diagnostics_with(rules, sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

fn restriction_rules() -> RuleConfig {
    let mut rules = RuleConfig::default();
    rules.configure("restriction", RuleLevel::Warn).unwrap();
    rules
}

fn codes(sql: &str) -> Vec<&'static str> {
    codes_with(restriction_rules(), sql)
}

#[test]
fn off_by_default() {
    for sql in [
        "UPDATE users SET name = 'x'",
        "DELETE FROM users",
        "INSERT INTO users VALUES (1, 'a', NULL)",
        "SELECT * FROM users",
        "SELECT id FROM users LIMIT 10",
    ] {
        assert!(codes_with(RuleConfig::default(), sql).is_empty(), "{sql}");
    }
}

#[test]
fn e0024_unfiltered_write() {
    assert_eq!(codes("UPDATE users SET name = 'x'"), ["E0024"]);
    assert_eq!(codes("DELETE FROM users"), ["E0024"]);
    assert!(codes("UPDATE users SET name = 'x' WHERE id = 1").is_empty());
    assert!(codes("DELETE FROM users WHERE true").is_empty());
}

#[test]
fn e0025_insert_without_columns() {
    assert_eq!(codes("INSERT INTO users VALUES (1, 'a', NULL)"), ["E0025"]);
    assert_eq!(
        codes("INSERT INTO archived SELECT id, name, email FROM users"),
        ["E0025"]
    );
    assert!(codes("INSERT INTO users (id, name) VALUES (1, 'a')").is_empty());
    assert!(!codes("INSERT INTO users DEFAULT VALUES").contains(&"E0025"));
}

#[test]
fn e0026_select_star() {
    assert_eq!(codes("SELECT * FROM users"), ["E0026"]);
    assert_eq!(codes("SELECT u.* FROM users u"), ["E0026"]);
    assert_eq!(
        codes("SELECT id, name, email FROM users UNION ALL SELECT * FROM archived"),
        ["E0026"]
    );
    assert_eq!(
        codes("INSERT INTO archived (id, name, email) SELECT * FROM users"),
        ["E0026"]
    );
    // Only the result columns: a star inside a subquery or EXISTS is fine
    assert!(codes("SELECT id FROM (SELECT * FROM users) s").is_empty());
    assert!(codes(
        "SELECT id FROM users u WHERE EXISTS (SELECT * FROM archived a WHERE a.id = u.id)"
    )
    .is_empty());
    assert!(codes("SELECT count(*) FROM users").is_empty());
}

#[test]
fn e0028_limit_without_order_by() {
    for sql in [
        "SELECT id FROM users LIMIT 20 OFFSET 40",
        "SELECT id FROM users OFFSET 40",
        "SELECT id FROM users FETCH FIRST 5 ROWS ONLY",
        "SELECT id FROM (SELECT id FROM users LIMIT 5) s ORDER BY id",
        "WITH s AS (SELECT id FROM users LIMIT 5) SELECT id FROM s ORDER BY id",
        "SELECT id FROM users UNION ALL SELECT id FROM archived LIMIT 5",
        // Once, though the INSERT is checked as a statement of its own too
        "WITH s AS (SELECT 1) INSERT INTO users (id, name) SELECT id, name FROM archived LIMIT 5",
    ] {
        assert_eq!(codes(sql), ["E0028"], "{sql}");
    }
    for sql in [
        "SELECT id FROM users ORDER BY id LIMIT 20 OFFSET 40",
        "SELECT id FROM users UNION ALL SELECT id FROM archived ORDER BY id LIMIT 5",
        "(SELECT id FROM users ORDER BY id) LIMIT 5",
        "SELECT id FROM users LIMIT ALL",
        // Which rows an EXISTS subquery returns doesn't matter
        "SELECT id FROM users u WHERE EXISTS (SELECT 1 FROM archived a WHERE a.id = u.id LIMIT 1)",
        "SELECT id FROM users WHERE NOT EXISTS ((SELECT 1 FROM archived LIMIT 1))",
    ] {
        assert!(codes(sql).is_empty(), "{sql}");
    }
    // A subquery of the EXISTS query is a query of its own
    assert_eq!(
        codes(
            "SELECT id FROM users WHERE EXISTS (SELECT 1 FROM (SELECT id FROM archived LIMIT 5) a)"
        ),
        ["E0028"]
    );
}

#[test]
fn e0028_has_a_location() {
    for (sql, keyword, column) in [
        ("SELECT id FROM users\nLIMIT 20 OFFSET 40", "LIMIT", 7),
        ("SELECT id FROM users\nOFFSET 40", "OFFSET", 8),
        (
            "SELECT 1;\nSELECT id FROM users FETCH FIRST ROW ONLY",
            "FETCH FIRST",
            1,
        ),
    ] {
        let diagnostics = diagnostics_with(restriction_rules(), sql);
        assert_eq!(diagnostics.len(), 1, "{sql}");
        assert_eq!(
            diagnostics[0].message,
            format!("{keyword} without ORDER BY returns an arbitrary set of rows")
        );
        let span = diagnostics[0].span.expect("span");
        assert_eq!((span.line, span.column), (2, column), "{sql}");
    }
}

#[test]
fn single_rule_can_be_enabled() {
    let mut rules = RuleConfig::default();
    rules.configure("select-star", RuleLevel::Error).unwrap();
    assert_eq!(
        codes_with(rules, "SELECT * FROM users; DELETE FROM users"),
        ["E0026"]
    );
}
