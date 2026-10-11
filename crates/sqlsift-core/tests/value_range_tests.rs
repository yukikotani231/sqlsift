//! E0017 value-out-of-range: literals a column can't hold in INSERT VALUES and
//! UPDATE SET. Every PostgreSQL query reported here is rejected by PostgreSQL 16;
//! every query asserted clean is accepted.

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE r (
    id INT PRIMARY KEY, name VARCHAR(5), code CHAR(3), age SMALLINT, big BIGINT,
    price NUMERIC(5,2), whole NUMERIC(3), frac NUMERIC(2,2), note TEXT, tiny TINYINT
);
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

#[track_caller]
fn assert_reports(sql: &str) {
    assert_eq!(codes_in(SqlDialect::PostgreSQL, sql), ["E0017"], "{sql}");
}

#[track_caller]
fn assert_clean(sql: &str) {
    assert_eq!(
        codes_in(SqlDialect::PostgreSQL, sql),
        Vec::<&str>::new(),
        "{sql}"
    );
}

#[test]
fn string_too_long() {
    assert_reports("INSERT INTO r (id, name) VALUES (1, 'toolong')");
    assert_reports("INSERT INTO r (id, code) VALUES (1, 'abcd')");
    assert_reports("INSERT INTO r VALUES (1, 'abcdef')");
    assert_reports("UPDATE r SET name = 'xyzxyz' WHERE id = 1");
    assert_reports("INSERT INTO r (id, name) VALUES (1, 'it''s!!')");
}

#[test]
fn string_that_fits() {
    // Excess trailing spaces are truncated; length counts characters, not bytes
    assert_clean("INSERT INTO r (id, name) VALUES (1, 'abcde   ')");
    assert_clean("INSERT INTO r (id, code) VALUES (1, 'ab  ')");
    assert_clean("INSERT INTO r (id, name) VALUES (1, 'äöüßé')");
    assert_clean("INSERT INTO r (id, name) VALUES (1, 'it''s ')");
    assert_clean("INSERT INTO r (id, note) VALUES (1, 'any length at all')");
}

#[test]
fn integer_out_of_range() {
    assert_reports("INSERT INTO r (id, age) VALUES (1, 32768)");
    assert_reports("INSERT INTO r (id, age) VALUES (1, -32769)");
    assert_reports("INSERT INTO r (id, age) VALUES (1, (-32769))");
    assert_reports("INSERT INTO r (id, big) VALUES (1, 9223372036854775808)");
    assert_reports("UPDATE r SET age = 40000");
}

#[test]
fn integer_in_range() {
    assert_clean("INSERT INTO r (id, age) VALUES (1, -32768)");
    // Rounded to 32767
    assert_clean("INSERT INTO r (id, age) VALUES (1, 32767.4)");
    // Comparisons are valid whatever the value
    assert_clean("SELECT id FROM r WHERE age = 99999");
    assert_clean("INSERT INTO r (id, age) VALUES (1, 40000 - 10000)");
}

#[test]
fn decimal_overflow() {
    assert_reports("INSERT INTO r (id, price) VALUES (1, 1234.5)");
    assert_reports("INSERT INTO r (id, whole) VALUES (1, 1000)");
    assert_reports("INSERT INTO r (id, frac) VALUES (1, 1.5)");
    assert_clean("INSERT INTO r (id, price) VALUES (1, 999.994)");
    assert_clean("INSERT INTO r (id, whole) VALUES (1, 999.4)");
    assert_clean("INSERT INTO r (id, frac) VALUES (1, 0.5)");
    assert_clean("INSERT INTO r (id, frac) VALUES (1, -0.99)");
}

#[test]
fn mysql_strict_mode() {
    let mysql = |sql| codes_in(SqlDialect::MySQL, sql);
    assert_eq!(
        mysql("INSERT INTO r (id, name) VALUES (1, \"toolong\")"),
        ["E0017"]
    );
    assert_eq!(mysql("INSERT INTO r (id, tiny) VALUES (1, 300)"), ["E0017"]);
    // The column may be UNSIGNED, which the catalog doesn't record
    assert!(mysql("INSERT INTO r (id, tiny) VALUES (1, 255)").is_empty());
    assert!(mysql("INSERT INTO r (id, age) VALUES (1, 65535)").is_empty());
}

#[test]
fn sqlite_stores_any_value() {
    let sqlite = |sql| codes_in(SqlDialect::SQLite, sql);
    assert!(sqlite("INSERT INTO r (id, name) VALUES (1, 'toolong')").is_empty());
    assert!(sqlite("INSERT INTO r (id, age) VALUES (1, 99999)").is_empty());
}
