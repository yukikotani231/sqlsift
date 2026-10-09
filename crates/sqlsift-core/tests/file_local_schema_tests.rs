// Tables, views and types created (or altered, or dropped) inside a query file are
// visible to the later statements of that file, and only to them (issue #83).
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const SCHEMA: &str = r"
    CREATE TABLE customer (
        customer_id INTEGER PRIMARY KEY,
        first_name TEXT NOT NULL
    );
    CREATE TABLE payment (
        payment_id INTEGER PRIMARY KEY,
        customer_id INTEGER NOT NULL REFERENCES customer(customer_id),
        amount NUMERIC(5, 2) NOT NULL
    );
";

fn catalog(schema: &str, dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    builder.build().0
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    let catalog = catalog(SCHEMA, SqlDialect::PostgreSQL);
    Analyzer::new(&catalog).analyze(sql)
}

fn assert_valid(sql: &str) {
    let diagnostics = analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics for `{sql}`, got: {diagnostics:#?}"
    );
}

fn kinds(diagnostics: &[Diagnostic]) -> Vec<DiagnosticKind> {
    diagnostics.iter().map(|d| d.kind).collect()
}

#[test]
fn temp_table_as_select_is_visible_to_later_statements() {
    assert_valid(
        "CREATE TEMP TABLE tmp_top AS SELECT customer_id, sum(amount) AS total FROM payment GROUP BY 1;
         SELECT c.first_name, t.total FROM tmp_top t JOIN customer c USING (customer_id) ORDER BY t.total DESC;
         DROP TABLE tmp_top;",
    );
}

#[test]
fn issue_83_repro_against_pagila() {
    let schema = include_str!("../../../tests/fixtures/real-world/pagila-schema.sql");
    let catalog = catalog(schema, SqlDialect::PostgreSQL);
    let diagnostics = Analyzer::new(&catalog).analyze(
        "CREATE TEMP TABLE tmp_top AS SELECT customer_id, sum(amount) AS total FROM payment GROUP BY 1;
         SELECT c.first_name, t.total FROM tmp_top t JOIN customer c USING (customer_id) ORDER BY t.total DESC;
         DROP TABLE tmp_top;",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn temporary_table_with_column_definitions() {
    assert_valid(
        "CREATE TEMPORARY TABLE scratch (id INTEGER NOT NULL, note TEXT);
         INSERT INTO scratch (id, note) VALUES (1, 'x');
         SELECT id, note FROM scratch WHERE id = 1;",
    );
    assert_valid(
        "CREATE TABLE scratch (id INTEGER, note TEXT);
         UPDATE scratch SET note = 'y' WHERE id = 1;
         DELETE FROM scratch WHERE id = 1;",
    );
}

#[test]
fn typos_against_file_local_table_are_reported() {
    let diagnostics = analyze(
        "CREATE TEMP TABLE tmp_top AS SELECT customer_id, sum(amount) AS total FROM payment GROUP BY 1;
         SELECT titel FROM tmp_top;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
    assert_eq!(diagnostics[0].span.map(|s| s.line), Some(2));
}

#[test]
fn file_local_column_types_are_checked() {
    let diagnostics = analyze(
        "CREATE TEMP TABLE scratch (id INTEGER NOT NULL, note TEXT);
         INSERT INTO scratch (id, note) VALUES ('abc', 'x');
         INSERT INTO scratch (id, note) VALUES (NULL, 'x');",
    );
    assert_eq!(
        kinds(&diagnostics),
        vec![
            DiagnosticKind::TypeMismatch,
            DiagnosticKind::PotentialNullViolation
        ]
    );
}

#[test]
fn ctas_query_is_still_analyzed() {
    let diagnostics = analyze("CREATE TEMP TABLE t AS SELECT custmer_id FROM payment;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
    let diagnostics = analyze("CREATE TEMP TABLE t AS SELECT customer_id FROM paymnt;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
}

#[test]
fn table_is_not_visible_before_it_is_created() {
    let diagnostics = analyze(
        "SELECT id FROM scratch;
         CREATE TEMP TABLE scratch (id INTEGER);",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
}

#[test]
fn dropped_table_is_no_longer_visible() {
    let diagnostics = analyze(
        "CREATE TEMP TABLE scratch (id INTEGER);
         SELECT id FROM scratch;
         DROP TABLE scratch;
         SELECT id FROM scratch;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    assert_eq!(diagnostics[0].span.map(|s| s.line), Some(4));
}

#[test]
fn alter_table_in_query_file_changes_columns() {
    assert_valid(
        "CREATE TEMP TABLE scratch (id INTEGER);
         ALTER TABLE scratch ADD COLUMN note TEXT;
         SELECT id, note FROM scratch;",
    );
    // Also for schema tables, within the file
    assert_valid(
        "ALTER TABLE customer ADD COLUMN email TEXT;
         SELECT email FROM customer;",
    );
    let diagnostics = analyze(
        "ALTER TABLE customer RENAME COLUMN first_name TO given_name;
         SELECT first_name FROM customer;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn view_created_in_query_file_is_visible() {
    assert_valid(
        "CREATE VIEW big_payers AS SELECT customer_id, sum(amount) AS total FROM payment GROUP BY 1;
         SELECT customer_id, total FROM big_payers;",
    );
    let diagnostics = analyze(
        "CREATE VIEW big_payers AS SELECT customer_id FROM payment;
         SELECT total FROM big_payers;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn view_query_is_analyzed() {
    let diagnostics = analyze("CREATE VIEW v AS SELECT amout FROM payment;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn ctas_with_unknown_columns_does_not_report_its_columns() {
    // The columns of `t` can't be inferred (`*` over an unknown table): referencing
    // them must not produce false positives, only the missing table is reported
    let diagnostics = analyze(
        "CREATE TEMP TABLE t AS SELECT * FROM missing_table;
         SELECT anything FROM t;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    assert_eq!(diagnostics[0].span.map(|s| s.line), Some(1));

    let diagnostics = analyze(
        "CREATE TEMP TABLE t AS SELECT * FROM missing_table;
         DROP TABLE t;
         SELECT anything FROM t;",
    );
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::TableNotFound, DiagnosticKind::TableNotFound]
    );
}

#[test]
fn file_local_tables_do_not_leak_into_other_analyses() {
    let catalog = catalog(SCHEMA, SqlDialect::PostgreSQL);
    let mut analyzer = Analyzer::new(&catalog);
    assert!(analyzer
        .analyze("CREATE TEMP TABLE scratch (id INTEGER); SELECT id FROM scratch;")
        .is_empty());
    // Same analyzer, next file
    let diagnostics = analyzer.analyze("SELECT id FROM scratch;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    // A fresh analyzer over the same catalog
    let diagnostics = Analyzer::new(&catalog).analyze("SELECT id FROM scratch;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    // Schema changes in a file don't alter the shared catalog
    assert!(analyzer
        .analyze("DROP TABLE customer; ALTER TABLE payment DROP COLUMN amount;")
        .is_empty());
    assert!(analyzer
        .analyze("SELECT first_name FROM customer; SELECT amount FROM payment;")
        .is_empty());
}

#[test]
fn file_local_tables_work_with_statement_by_statement_parsing() {
    // A syntax error elsewhere makes the file parse statement by statement
    let diagnostics = analyze(
        "CREATE TEMP TABLE scratch (id INTEGER);
         SELECT FROM WHERE;
         SELECT id FROM scratch;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ParseError]);
}

#[test]
fn file_local_tables_in_mysql_and_sqlite() {
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        let catalog = catalog(SCHEMA, dialect);
        let diagnostics = Analyzer::with_dialect(&catalog, dialect).analyze(
            "CREATE TEMPORARY TABLE tmp_top AS SELECT customer_id, sum(amount) AS total FROM payment GROUP BY customer_id;
             SELECT t.total FROM tmp_top t JOIN customer c ON c.customer_id = t.customer_id;
             SELECT titel FROM tmp_top;",
        );
        assert_eq!(
            kinds(&diagnostics),
            vec![DiagnosticKind::ColumnNotFound],
            "{dialect:?}: {diagnostics:#?}"
        );
    }
}

// Issue #117: PostgreSQL scripts

#[test]
fn unlogged_table_is_visible_to_later_statements() {
    assert_valid(
        "CREATE UNLOGGED TABLE stage_pay (LIKE payment INCLUDING ALL);
         SELECT amount FROM stage_pay;",
    );
    let diagnostics = analyze(
        "CREATE UNLOGGED TABLE stage_pay (LIKE payment INCLUDING ALL);
         SELECT amont FROM stage_pay;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn create_table_as_with_no_data_is_visible_to_later_statements() {
    assert_valid(
        "CREATE TABLE IF NOT EXISTS t3 AS SELECT * FROM customer WITH NO DATA;
         SELECT first_name FROM t3;
         CREATE TABLE t4 AS SELECT customer_id FROM payment WITH DATA;
         SELECT customer_id FROM t4;",
    );
    let diagnostics = analyze(
        "CREATE TABLE t3 AS SELECT * FROM customer WITH NO DATA;
         SELECT last_name FROM t3;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
    // Locations in the retried statement are kept
    let diagnostics =
        analyze("SELECT 1;\nCREATE UNLOGGED TABLE t5 AS SELECT nope FROM customer WITH NO DATA;");
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.line, span.column), (2, 36));
}

#[test]
fn table_whose_create_failed_to_parse_says_so() {
    let diagnostics = analyze(
        "CREATE TABLE t2 (id INTEGER);
         CREATE TABLE t3 (id INTEGER) bogus syntax here;
         SELECT id FROM t3;",
    );
    assert_eq!(
        kinds(&diagnostics),
        vec![DiagnosticKind::ParseError, DiagnosticKind::TableNotFound]
    );
    let help = diagnostics[1].help.as_deref().unwrap();
    assert!(
        help.contains("'t3' on line 2 could not be parsed"),
        "{help}"
    );
    // Only for the later statements
    let diagnostics = analyze(
        "SELECT id FROM t3;
         CREATE TABLE t3 (id INTEGER) bogus syntax here;",
    );
    let help = diagnostics[0].help.as_deref().unwrap();
    assert!(!help.contains("could not be parsed"), "{help}");
}

#[test]
fn select_into_creates_a_table() {
    assert_valid(
        "SELECT customer_id, first_name INTO TEMP tmp_c FROM customer;
         SELECT first_name FROM tmp_c;
         SELECT customer_id INTO TEMPORARY TABLE tmp_p FROM payment;
         SELECT customer_id FROM tmp_p;
         SELECT 1 AS one INTO UNLOGGED tmp_o;
         SELECT one FROM tmp_o;
         SELECT * INTO plain FROM payment;
         SELECT amount FROM plain;",
    );
    let diagnostics = analyze(
        "SELECT customer_id INTO TEMP tmp_c FROM customer;
         SELECT first_name FROM tmp_c;",
    );
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::ColumnNotFound]);
}

#[test]
fn set_search_path_is_followed() {
    let schema = "
        CREATE SCHEMA analytics;
        CREATE TABLE analytics.daily_active (day DATE, dau INTEGER);
        CREATE TABLE users (id INTEGER);
    ";
    let catalog = catalog(schema, SqlDialect::PostgreSQL);
    let analyze = |sql: &str| Analyzer::new(&catalog).analyze(sql);
    for sql in [
        "SET search_path TO analytics, public;
         SELECT dau FROM daily_active JOIN users ON users.id = dau;",
        "SET search_path = analytics;
         SELECT dau FROM daily_active;",
        "SET LOCAL search_path TO \"$user\", analytics;
         SELECT dau FROM daily_active;",
        "SET search_path = 'analytics, public';
         SELECT dau FROM daily_active;",
        // Tables created afterwards go in the first schema
        "SET search_path TO analytics;
         CREATE TABLE fresh (x INTEGER);
         SELECT x FROM analytics.fresh;",
    ] {
        let diagnostics = analyze(sql);
        assert!(diagnostics.is_empty(), "{sql}: {diagnostics:#?}");
    }
    // Only for the rest of the file
    assert_eq!(
        kinds(&analyze(
            "SELECT dau FROM daily_active;
             SET search_path TO analytics;
             SELECT nope FROM daily_active;
             SET search_path TO DEFAULT;
             SELECT dau FROM daily_active;"
        )),
        vec![
            DiagnosticKind::TableNotFound,
            DiagnosticKind::ColumnNotFound,
            DiagnosticKind::TableNotFound
        ]
    );
    // A schema left out of the path is not searched
    assert_eq!(
        kinds(&analyze(
            "SET search_path TO analytics;
             SELECT id FROM users;"
        )),
        vec![DiagnosticKind::TableNotFound]
    );
    assert_eq!(analyze("SELECT dau FROM daily_active").len(), 1);
}

// Issue #124: E0001 help mentions when the table was dropped earlier in the file

#[test]
fn dropped_table_help_mentions_line_of_drop() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);
SELECT id FROM tmp_ev;
DROP TABLE tmp_ev;
SELECT id FROM tmp_ev;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(d.span.map(|s| s.line), Some(4));
    assert_eq!(
        d.help.as_deref(),
        Some("'tmp_ev' was dropped at line 3 of this file")
    );
}

#[test]
fn dropped_table_help_three_statements_line_3() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);

DROP TABLE tmp_ev;

SELECT id FROM tmp_ev;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(d.span.map(|s| s.line), Some(5));
    assert_eq!(
        d.help.as_deref(),
        Some("'tmp_ev' was dropped at line 3 of this file")
    );
}

#[test]
fn dropped_table_help_multiline_drop() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);

DROP TABLE
    tmp_ev;

SELECT id FROM tmp_ev;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(
        d.help.as_deref(),
        Some("'tmp_ev' was dropped at line 3 of this file")
    );
}

#[test]
fn table_never_present_preserves_standard_help() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);
SELECT id FROM tmp_ev;
DROP TABLE tmp_ev;
SELECT id FROM missing_table;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(d.span.map(|s| s.line), Some(4));
    let help = d.help.as_deref().unwrap();
    assert!(
        !help.contains("was dropped at line"),
        "Expected standard help, got: {help}"
    );
    assert_eq!(
        help,
        "Check that the table exists in your schema definition; run `sqlsift schema <schema files>` to list the tables that were loaded"
    );
}

#[test]
fn dropped_view_help_mentions_line_of_drop() {
    let sql = "CREATE VIEW tmp_v AS SELECT customer_id FROM payment;
SELECT customer_id FROM tmp_v;
DROP VIEW tmp_v;
SELECT customer_id FROM tmp_v;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(d.span.map(|s| s.line), Some(4));
    assert_eq!(
        d.help.as_deref(),
        Some("'tmp_v' was dropped at line 3 of this file")
    );
}

#[test]
fn dropped_table_help_case_insensitive() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);
DROP TABLE TMP_EV;
SELECT id FROM tmp_ev;";
    let diagnostics = analyze(sql);
    assert_eq!(kinds(&diagnostics), vec![DiagnosticKind::TableNotFound]);
    let d = &diagnostics[0];
    assert_eq!(
        d.help.as_deref(),
        Some("'tmp_ev' was dropped at line 2 of this file")
    );
}

#[test]
fn dropped_and_recreated_table_succeeds() {
    let sql = "CREATE TEMP TABLE tmp_ev (id INTEGER);
DROP TABLE tmp_ev;
CREATE TEMP TABLE tmp_ev (id INTEGER);
SELECT id FROM tmp_ev;";
    let diagnostics = analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics, got: {diagnostics:#?}"
    );
}
