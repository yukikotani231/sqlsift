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

// ---------------------------------------------------------------------------
// Redshift
// ---------------------------------------------------------------------------

#[test]
fn redshift_table_attributes_are_parsed() {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::Redshift);
    builder
        .parse(
            "CREATE TABLE lineitem (
                l_orderkey INT8 NOT NULL DISTKEY,
                l_shipdate DATE NOT NULL SORTKEY,
                l_comment VARCHAR(44) NOT NULL ENCODE zstd,
                l_quantity DECIMAL(12,2) ENCODE AZ64
            ) DISTSTYLE KEY;
            CREATE TABLE orders (o_orderkey INT8 NOT NULL, o_orderdate DATE)
                DISTKEY (o_orderkey) COMPOUND SORTKEY (o_orderdate, o_orderkey) BACKUP NO;
            CREATE TABLE nation (n_nationkey INT4, sortkey INT, distkey INT)
                DISTSTYLE ALL INTERLEAVED SORTKEY (n_nationkey);
            CREATE TEMP TABLE events (id BIGINT IDENTITY(1, 1) NOT NULL, name VARCHAR(max))
                SORTKEY AUTO ENCODE AUTO;
            CREATE TABLE dim_calendar DISTSTYLE ALL SORTKEY (id) AS SELECT 1 AS id, 'x' AS label;",
        )
        .unwrap();
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "{warnings:?}");
    let check = |sql: &str| -> Vec<DiagnosticKind> {
        Analyzer::with_dialect(&catalog, SqlDialect::Redshift)
            .analyze(sql)
            .into_iter()
            .map(|d| d.kind)
            .collect()
    };
    assert!(check(
        "SELECT l_orderkey, l_comment, l_quantity * 2, o_orderdate, sortkey, distkey, label \
         FROM lineitem JOIN orders ON o_orderkey = l_orderkey \
         CROSS JOIN nation CROSS JOIN dim_calendar \
         WHERE l_shipdate > '2020-01-01'"
    )
    .is_empty());
    // Columns keep their types, so their misuse is still found
    assert_eq!(
        check("SELECT 1 FROM lineitem WHERE l_quantity = 'abc'"),
        vec![DiagnosticKind::TypeMismatch]
    );
    assert_eq!(
        check("SELECT l_nope FROM lineitem"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // An IDENTITY column has a value without being given one
    assert!(check("INSERT INTO events (name) VALUES ('a')").is_empty());
}

#[test]
fn redshift_date_parts_and_niladic_functions() {
    let sql = "SELECT date_part(h, created_at), datepart(epoch, created_at), \
               datediff(d, created_at, sysdate), dateadd(mins, 5, created_at), \
               date_trunc(qtr, created_at), sysdate - created_at, getdate(), current_user \
               FROM orders WHERE created_at < sysdate";
    assert!(check(SqlDialect::Redshift, sql).is_empty());
    // A name that isn't a date part is still a column
    assert_eq!(
        kinds(
            SqlDialect::Redshift,
            "SELECT date_part(hr_of_day, created_at) FROM orders"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // `sysdate` is only a function in Redshift
    assert_eq!(
        kinds(SqlDialect::PostgreSQL, "SELECT sysdate FROM orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

// ---------------------------------------------------------------------------
// Databricks
// ---------------------------------------------------------------------------

fn databricks(schema: &str, sql: &str) -> Vec<DiagnosticKind> {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::Databricks);
    builder.parse(schema).unwrap();
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "{warnings:?}");
    Analyzer::with_dialect(&catalog, SqlDialect::Databricks)
        .analyze(sql)
        .into_iter()
        .map(|d| d.kind)
        .collect()
}

#[test]
fn databricks_complex_types_and_table_options() {
    let schema = "CREATE TABLE events (
            id BIGINT,
            n LONG,
            s SHORT,
            b BYTE,
            tags ARRAY<STRING>,
            props MAP<STRING, INT>,
            nested MAP<STRING, ARRAY<STRUCT<k: STRING, v: DECIMAL(10, 2)>>>,
            info STRUCT<name: STRING, address: STRUCT<city: STRING, zip: INT>>,
            items ARRAY<STRUCT<sku STRING, qty INT>>
        ) USING DELTA PARTITIONED BY (id) CLUSTER BY (n)
          LOCATION 's3://bucket/events' TBLPROPERTIES ('delta.appendOnly' = 'true');
        CREATE TABLE logs (msg STRING) USING PARQUET PARTITIONED BY (dt STRING) OPTIONS (path 'x');";
    assert!(databricks(
        schema,
        "SELECT id, n + 1, s * 2, b - 1, tags[0], props['k'], nested['a'], info.name, \
         events.info.address.city FROM events"
    )
    .is_empty());
    // Hive-format partition columns are columns of the table
    assert!(databricks(schema, "SELECT msg, dt FROM logs").is_empty());
    // LONG is numeric
    assert_eq!(
        databricks(schema, "SELECT id FROM events WHERE n = 'abc'"),
        vec![DiagnosticKind::TypeMismatch]
    );
    // A misspelled alias or column is still reported
    assert_eq!(
        databricks(schema, "SELECT evnts.id FROM events"),
        vec![DiagnosticKind::TableNotFound]
    );
    assert_eq!(
        databricks(schema, "SELECT nope FROM events"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn databricks_lateral_view() {
    let schema = "CREATE TABLE t (id INT, tags ARRAY<STRING>, m MAP<STRING, INT>);";
    assert!(databricks(
        schema,
        "SELECT id, tag, x.tag FROM t LATERAL VIEW explode(tags) x AS tag"
    )
    .is_empty());
    assert!(databricks(
        schema,
        "SELECT id, k, v, p.pos FROM t LATERAL VIEW OUTER explode(m) e AS k, v \
         LATERAL VIEW posexplode(tags) p AS pos, val"
    )
    .is_empty());
    // The generator's argument is checked, and so are names that aren't its columns
    assert_eq!(
        databricks(
            schema,
            "SELECT id FROM t LATERAL VIEW explode(tgs) x AS tag"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_eq!(
        databricks(
            schema,
            "SELECT x.nope FROM t LATERAL VIEW explode(tags) x AS tag"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn long_is_not_numeric_outside_databricks() {
    // MySQL's LONG is MEDIUMTEXT
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder.parse("CREATE TABLE t (body LONG);").unwrap();
    let catalog = builder.build().0;
    let diagnostics = Analyzer::with_dialect(&catalog, SqlDialect::MySQL)
        .analyze("SELECT body FROM t WHERE body = 'abc'");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}
