//! Data warehouse dialects (alpha): Snowflake, BigQuery, Redshift, Databricks

use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::{Analyzer, Diagnostic, DiagnosticKind, SqlDialect};

const SCHEMA: &str = "
CREATE TABLE orders (id INTEGER NOT NULL, customer_id INTEGER, amount DECIMAL(10, 2), created_at TIMESTAMP);
CREATE TABLE customers (id INTEGER NOT NULL, name TEXT);
";

fn check(dialect: SqlDialect, sql: &str) -> Vec<Diagnostic> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(SCHEMA).unwrap();
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect).analyze(sql)
}

fn kinds(dialect: SqlDialect, sql: &str) -> Vec<DiagnosticKind> {
    check(dialect, sql).into_iter().map(|d| d.kind).collect()
}

const WAREHOUSES: [SqlDialect; 4] = [
    SqlDialect::Snowflake,
    SqlDialect::BigQuery,
    SqlDialect::Redshift,
    SqlDialect::Databricks,
];

#[test]
fn unknown_table_and_column_are_reported_in_every_warehouse_dialect() {
    for dialect in WAREHOUSES {
        assert_eq!(
            kinds(dialect, "SELECT id FROM order_items"),
            vec![DiagnosticKind::TableNotFound],
            "{dialect}"
        );
        assert_eq!(
            kinds(dialect, "SELECT o.total FROM orders o"),
            vec![DiagnosticKind::ColumnNotFound],
            "{dialect}"
        );
        assert!(
            check(
                dialect,
                "SELECT o.id, c.name FROM orders o JOIN customers c ON c.id = o.customer_id"
            )
            .is_empty(),
            "{dialect}"
        );
    }
}

#[test]
fn qualify_is_checked_and_sees_output_aliases() {
    for dialect in [
        SqlDialect::Snowflake,
        SqlDialect::BigQuery,
        SqlDialect::Databricks,
    ] {
        let ok =
            "SELECT id, ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY created_at) AS rn \
                  FROM orders QUALIFY rn = 1";
        assert!(check(dialect, ok).is_empty(), "{dialect}");
        let bad = "SELECT id FROM orders \
                   QUALIFY ROW_NUMBER() OVER (PARTITION BY customer ORDER BY created_at) = 1";
        assert_eq!(
            kinds(dialect, bad),
            vec![DiagnosticKind::ColumnNotFound],
            "{dialect}"
        );
    }
}

#[test]
fn lateral_column_aliases() {
    let sql =
        "SELECT amount * 2 AS doubled, doubled + 1 AS plus_one FROM orders WHERE doubled > 10";
    for dialect in [
        SqlDialect::Snowflake,
        SqlDialect::Redshift,
        SqlDialect::Databricks,
    ] {
        assert!(check(dialect, sql).is_empty(), "{dialect}");
    }
    // PostgreSQL has no lateral column aliases
    assert!(!check(SqlDialect::PostgreSQL, sql).is_empty());
    // A name that is neither a column nor an alias is still reported
    assert_eq!(
        kinds(
            SqlDialect::Snowflake,
            "SELECT amount AS a FROM orders WHERE b > 1"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn having_sees_output_aliases_in_warehouses() {
    let sql = "SELECT customer_id, COUNT(*) AS n FROM orders GROUP BY customer_id HAVING n > 1";
    for dialect in WAREHOUSES {
        assert!(check(dialect, sql).is_empty(), "{dialect}");
    }
}

#[test]
fn warehouse_syntax_parses() {
    let cases = [
        (
            SqlDialect::Snowflake,
            "SELECT o.id, f.value:sku::varchar FROM orders o, LATERAL FLATTEN(input => o.items) f",
        ),
        (
            SqlDialect::Snowflake,
            "SELECT * EXCLUDE (amount) FROM orders",
        ),
        (
            SqlDialect::Snowflake,
            "SELECT IFF(amount > 1, 'x', 'y'), ZEROIFNULL(amount) FROM orders",
        ),
        (
            SqlDialect::BigQuery,
            "SELECT * EXCEPT (amount) FROM `orders`",
        ),
        (
            SqlDialect::BigQuery,
            "SELECT SAFE_CAST(amount AS INT64) FROM orders",
        ),
        (
            SqlDialect::BigQuery,
            "SELECT id FROM orders WHERE _PARTITIONTIME > '2024-01-01'",
        ),
        (SqlDialect::Redshift, "SELECT id::varchar FROM orders"),
        (SqlDialect::Databricks, "SELECT `id` FROM orders"),
    ];
    for (dialect, sql) in cases {
        let diagnostics = check(dialect, sql);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ParseError),
            "{dialect}: {sql}: {diagnostics:?}"
        );
    }
}

#[test]
fn snowflake_colon_paths_are_not_psql_variables() {
    // psql's `:var` rewriting applies to PostgreSQL and Redshift only
    let diagnostics = check(
        SqlDialect::Snowflake,
        "SELECT o.id FROM orders o WHERE o.amount:x = 1",
    );
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::ParseError),
        "{diagnostics:?}"
    );
}
