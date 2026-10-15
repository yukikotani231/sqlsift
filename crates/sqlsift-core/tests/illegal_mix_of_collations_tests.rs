//! E0036 illegal-mix-of-collations: Comparing or combining columns with
//! incompatible collations in MySQL.

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE a (
    id INT PRIMARY KEY,
    code VARCHAR(10) COLLATE utf8mb4_unicode_ci,
    title VARCHAR(100)
) DEFAULT CHARSET = utf8mb4 DEFAULT COLLATE = utf8mb4_unicode_ci;

CREATE TABLE b (
    id INT PRIMARY KEY,
    code VARCHAR(10) COLLATE utf8mb4_0900_ai_ci,
    title VARCHAR(100)
) DEFAULT CHARSET = utf8mb4 DEFAULT COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE c (
    id INT PRIMARY KEY,
    code VARCHAR(10) COLLATE utf8mb4_unicode_ci
) DEFAULT CHARSET = utf8mb4 DEFAULT COLLATE = utf8mb4_unicode_ci;

CREATE TABLE uncollated (
    id INT PRIMARY KEY,
    code VARCHAR(10)
);
";

fn mysql_analyzer(sql: &str) -> Vec<sqlsift_core::Diagnostic> {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder.parse(SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, SqlDialect::MySQL).analyze(sql)
}

fn mysql_codes(sql: &str) -> Vec<&'static str> {
    mysql_analyzer(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[track_caller]
fn assert_reports(sql: &str) {
    let diags = mysql_analyzer(sql);
    let found: Vec<_> = diags.iter().map(sqlsift_core::Diagnostic::code).collect();
    assert!(
        found.contains(&"E0036"),
        "expected E0036 for {sql}, got {found:?}"
    );
    let e0036 = diags.iter().find(|d| d.code() == "E0036").unwrap();
    assert!(
        e0036.message.contains("Illegal mix of collations"),
        "unexpected message: {}",
        e0036.message
    );
}

#[track_caller]
fn assert_clean(sql: &str) {
    assert_eq!(mysql_codes(sql), Vec::<&str>::new(), "{sql}");
}

#[test]
fn test_comparison_operators() {
    assert_reports("SELECT * FROM a, b WHERE a.code = b.code");
    assert_reports("SELECT * FROM a, b WHERE a.code <> b.code");
    assert_reports("SELECT * FROM a, b WHERE a.code < b.code");
    assert_reports("SELECT * FROM a, b WHERE a.code <= b.code");
    assert_reports("SELECT * FROM a, b WHERE a.code > b.code");
    assert_reports("SELECT * FROM a, b WHERE a.code >= b.code");
}

#[test]
fn test_join_conditions() {
    assert_reports("SELECT * FROM a JOIN b ON a.code = b.code");
    assert_reports("SELECT * FROM a JOIN b USING (code)");
}

#[test]
fn test_like_conditions() {
    assert_reports("SELECT * FROM a, b WHERE a.code LIKE b.code");
}

#[test]
fn test_in_subquery() {
    assert_reports("SELECT * FROM a WHERE a.code IN (SELECT b.code FROM b)");
}

#[test]
fn test_in_list() {
    assert_reports("SELECT * FROM a, b WHERE a.code IN (b.code)");
}

#[test]
fn test_case_expression() {
    assert_reports("SELECT CASE a.code WHEN b.code THEN 1 ELSE 0 END FROM a, b");
    assert_reports("SELECT CASE WHEN a.id = 1 THEN a.code ELSE b.code END FROM a, b");
}

#[test]
fn test_coalesce_and_ifnull() {
    assert_reports("SELECT COALESCE(a.code, b.code) FROM a, b");
    assert_reports("SELECT IFNULL(a.code, b.code) FROM a, b");
}

#[test]
fn test_set_operations() {
    assert_reports("SELECT a.code FROM a UNION SELECT b.code FROM b");
    assert_reports("SELECT a.code FROM a INTERSECT SELECT b.code FROM b");
    assert_reports("SELECT a.code FROM a EXCEPT SELECT b.code FROM b");
}

#[test]
fn test_same_collation_is_clean() {
    assert_clean("SELECT * FROM a JOIN c ON a.code = c.code");
    assert_clean("SELECT a.code FROM a UNION SELECT c.code FROM c");
}

#[test]
fn test_explicit_collate_is_clean() {
    assert_clean("SELECT * FROM a JOIN b ON a.code COLLATE utf8mb4_bin = b.code");
    assert_clean("SELECT * FROM a JOIN b ON a.code = b.code COLLATE utf8mb4_unicode_ci");
}

#[test]
fn test_literal_is_clean() {
    assert_clean("SELECT * FROM a WHERE a.code = 'foo'");
    assert_clean("SELECT * FROM a WHERE a.code IN ('foo', 'bar')");
}

#[test]
fn test_unknown_collation_is_clean() {
    assert_clean("SELECT * FROM a JOIN uncollated ON a.code = uncollated.code");
}

#[test]
fn test_non_mysql_is_clean() {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::PostgreSQL);
    builder.parse(SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    let diags = Analyzer::with_dialect(&catalog, SqlDialect::PostgreSQL)
        .analyze("SELECT * FROM a JOIN b ON a.code = b.code");
    let codes: Vec<_> = diags.iter().map(sqlsift_core::Diagnostic::code).collect();
    assert!(
        !codes.contains(&"E0036"),
        "PostgreSQL should not report E0036"
    );
}

#[test]
fn test_mysql_coercibility_silences() {
    const MORE_SCHEMA: &str = "
    CREATE TABLE t_latin (id INT, code VARCHAR(10) COLLATE latin1_general_ci);
    CREATE TABLE t_latin_bin (id INT, code VARCHAR(10) COLLATE latin1_bin);
    CREATE TABLE t_utf8 (id INT, code VARCHAR(10) COLLATE utf8_general_ci);
    CREATE TABLE t_utf8mb3 (id INT, code VARCHAR(10) COLLATE utf8mb3_general_ci);
    CREATE TABLE t_utf8mb4 (id INT, code VARCHAR(10) COLLATE utf8mb4_0900_ai_ci);
    CREATE TABLE t_explicit_charset (id INT, code VARCHAR(10) CHARACTER SET utf8mb4);
    ";
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder.parse(MORE_SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    let check_clean = |sql: &str| {
        let diags = Analyzer::with_dialect(&catalog, SqlDialect::MySQL).analyze(sql);
        let codes: Vec<_> = diags.iter().map(sqlsift_core::Diagnostic::code).collect();
        assert!(
            !codes.contains(&"E0036"),
            "expected no E0036 for {sql}, got {codes:?}"
        );
    };

    // utf8mb4 vs latin1 (Unicode side wins per coercibility rules)
    check_clean("SELECT * FROM t_utf8mb4 u JOIN t_latin l ON u.code = l.code");

    // utf8mb4 vs utf8mb3 (utf8mb4 superset converts)
    check_clean("SELECT * FROM t_utf8mb4 u JOIN t_utf8mb3 m ON u.code = m.code");

    // _bin vs _ci of same charset (uses _bin, no error)
    check_clean("SELECT * FROM t_latin l JOIN t_latin_bin b ON l.code = b.code");

    // utf8_general_ci vs utf8mb3_general_ci (same collation under two names)
    check_clean("SELECT * FROM t_utf8 u JOIN t_utf8mb3 m ON u.code = m.code");

    // Column with CHARACTER SET utf8mb4 and no COLLATE gets charset default (unknown collation)
    check_clean("SELECT * FROM t_utf8mb4 u JOIN t_explicit_charset e ON u.code = e.code");
}
