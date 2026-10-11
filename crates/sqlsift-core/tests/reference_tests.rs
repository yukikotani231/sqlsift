//! References to catalog tables, views and columns (`Analyzer::with_references`)

use sqlsift_core::analyzer::{Analyzer, ReferenceKind};
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Catalog;

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder
        .parse(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT);
             CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, total NUMERIC);
             CREATE VIEW big_orders AS SELECT id, total FROM orders WHERE total > 100;",
        )
        .unwrap();
    builder.build().0
}

/// `(relation, column)` of each reference: column is `""` for the relation
/// itself and `"*"` for all columns
fn references(sql: &str) -> Vec<(String, String)> {
    let catalog = catalog();
    let mut analyzer = Analyzer::new(&catalog).with_references();
    analyzer.analyze(sql);
    analyzer
        .references()
        .iter()
        .map(|r| {
            let column = match &r.kind {
                ReferenceKind::Relation => String::new(),
                ReferenceKind::Column(c) => c.clone(),
                ReferenceKind::AllColumns => "*".to_string(),
            };
            (r.relation.to_string(), column)
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(r, c)| ((*r).to_string(), (*c).to_string()))
        .collect()
}

#[test]
fn qualified_and_unqualified_columns() {
    assert_eq!(
        references("SELECT u.name, total FROM users u JOIN orders o ON o.user_id = u.id"),
        pairs(&[
            ("public.users", "name"),
            ("public.orders", "total"),
            ("public.users", ""),
            ("public.orders", ""),
            ("public.orders", "user_id"),
            ("public.users", "id"),
        ])
    );
}

#[test]
fn columns_read_through_a_cte() {
    // The CTE's own columns are not catalog columns
    assert_eq!(
        references("WITH t AS (SELECT id AS uid FROM users) SELECT uid FROM t"),
        pairs(&[("public.users", "id"), ("public.users", "")])
    );
}

#[test]
fn views_and_stars() {
    assert_eq!(
        references("SELECT * FROM big_orders"),
        pairs(&[("public.big_orders", "*"), ("public.big_orders", "")])
    );
    assert_eq!(
        references("SELECT o.* FROM orders o, users"),
        pairs(&[
            ("public.orders", "*"),
            ("public.orders", ""),
            ("public.users", ""),
        ])
    );
}

#[test]
fn write_targets() {
    assert_eq!(
        references("INSERT INTO users (id, name) VALUES (1, 'a')"),
        pairs(&[
            ("public.users", ""),
            ("public.users", "id"),
            ("public.users", "name"),
        ])
    );
    assert_eq!(
        references("UPDATE orders SET total = 0 WHERE user_id = 1"),
        pairs(&[
            ("public.orders", ""),
            ("public.orders", "total"),
            ("public.orders", "user_id"),
        ])
    );
}

#[test]
fn unknown_names_are_not_references() {
    assert_eq!(references("SELECT nope FROM missing"), pairs(&[]));
    assert_eq!(
        references("SELECT nope FROM users"),
        pairs(&[("public.users", "")])
    );
}

#[test]
fn off_by_default() {
    let catalog = catalog();
    let mut analyzer = Analyzer::new(&catalog);
    analyzer.analyze("SELECT id FROM users");
    assert!(analyzer.references().is_empty());
}

#[test]
fn spans_point_at_the_names() {
    let catalog = catalog();
    let mut analyzer = Analyzer::new(&catalog).with_references();
    analyzer.analyze("SELECT 1;\nSELECT name\nFROM users");
    let spans: Vec<(usize, usize, usize)> = analyzer
        .references()
        .iter()
        .map(|r| (r.span.line, r.span.column, r.span.offset))
        .collect();
    assert_eq!(spans, [(2, 8, 17), (3, 6, 27)]);
}
