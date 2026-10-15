//! E0036 illegal-mix-of-collations: Incompatible collations in MySQL expressions.
//!
//! In MySQL, comparisons, LIKE, IN, JOIN ON/USING, CASE, COALESCE, and set operations
//! combining two column references with different collations fail at runtime with
//! error 1267 (Illegal mix of collations).

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const MYSQL_SCHEMA: &str = "
CREATE TABLE t_general (
    id INT PRIMARY KEY,
    name VARCHAR(50) COLLATE utf8mb4_general_ci,
    tag VARCHAR(50) COLLATE utf8mb4_general_ci,
    no_coll VARCHAR(50)
);

CREATE TABLE t_unicode (
    id INT PRIMARY KEY,
    name VARCHAR(50) COLLATE utf8mb4_unicode_ci,
    tag VARCHAR(50) COLLATE utf8mb4_unicode_ci
);

CREATE TABLE t_bin (
    id INT PRIMARY KEY,
    name VARCHAR(50) COLLATE utf8mb4_bin
);

CREATE TABLE t_default_general (
    id INT PRIMARY KEY,
    val VARCHAR(50)
) DEFAULT CHARSET = utf8mb4 DEFAULT COLLATE = utf8mb4_general_ci;

CREATE TABLE t_default_unicode (
    id INT PRIMARY KEY,
    val VARCHAR(50)
) DEFAULT CHARSET = utf8mb4 DEFAULT COLLATE = utf8mb4_unicode_ci;

CREATE TABLE t_using1 (
    id INT PRIMARY KEY,
    code VARCHAR(50) COLLATE utf8mb4_general_ci
);

CREATE TABLE t_using2 (
    id INT PRIMARY KEY,
    code VARCHAR(50) COLLATE utf8mb4_unicode_ci
);
";

const PG_SCHEMA: &str = r#"
CREATE TABLE t_pg1 (
    id INT PRIMARY KEY,
    name TEXT COLLATE "en_US"
);
CREATE TABLE t_pg2 (
    id INT PRIMARY KEY,
    name TEXT COLLATE "C"
);
"#;

fn mysql_codes(sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder.parse(MYSQL_SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, SqlDialect::MySQL)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

fn pg_codes(sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::PostgreSQL);
    builder.parse(PG_SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, SqlDialect::PostgreSQL)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[track_caller]
fn assert_reports_e0036(sql: &str) {
    let found = mysql_codes(sql);
    assert!(
        found.contains(&"E0036"),
        "expected E0036 for {sql}, got {found:?}"
    );
}

#[track_caller]
fn assert_mysql_clean(sql: &str) {
    let found = mysql_codes(sql);
    assert!(
        !found.contains(&"E0036"),
        "expected no E0036 for {sql}, got {found:?}"
    );
}

#[test]
fn test_join_on_different_collations() {
    assert_reports_e0036("SELECT * FROM t_general g JOIN t_unicode u ON g.name = u.name");
    assert_mysql_clean("SELECT * FROM t_general g JOIN t_bin b ON g.name = b.name");
}

#[test]
fn test_join_using_different_collations() {
    assert_reports_e0036("SELECT * FROM t_using1 JOIN t_using2 USING (code)");
}

#[test]
fn test_comparisons_different_collations() {
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name = u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name != u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name <> u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name < u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name > u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name <= u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name >= u.name");
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name <=> u.name");
}

#[test]
fn test_like_different_collations() {
    assert_reports_e0036("SELECT * FROM t_general g, t_unicode u WHERE g.name LIKE u.name");
}

#[test]
fn test_in_list_different_collations() {
    assert_reports_e0036(
        "SELECT * FROM t_general g, t_unicode u WHERE g.name IN (u.name, 'static')",
    );
}

#[test]
fn test_in_subquery_different_collations() {
    assert_reports_e0036("SELECT * FROM t_general WHERE name IN (SELECT name FROM t_unicode)");
}

#[test]
fn test_case_different_collations() {
    assert_reports_e0036(
        "SELECT CASE WHEN id > 1 THEN g.name ELSE u.name END FROM t_general g, t_unicode u",
    );
    assert_reports_e0036(
        "SELECT CASE g.name WHEN u.name THEN 1 ELSE 0 END FROM t_general g, t_unicode u",
    );
}

#[test]
fn test_coalesce_different_collations() {
    assert_reports_e0036("SELECT COALESCE(g.name, u.name) FROM t_general g, t_unicode u");
}

#[test]
fn test_set_operations_different_collations() {
    assert_reports_e0036("SELECT name FROM t_general UNION SELECT name FROM t_unicode");
    assert_reports_e0036("SELECT name FROM t_general UNION ALL SELECT name FROM t_unicode");
}

#[test]
fn test_table_default_collation_inheritance() {
    assert_reports_e0036(
        "SELECT * FROM t_default_general dg JOIN t_default_unicode du ON dg.val = du.val",
    );
    assert_reports_e0036("SELECT * FROM t_default_general dg, t_unicode u WHERE dg.val = u.name");
}

#[test]
fn test_literals_are_silent() {
    assert_mysql_clean("SELECT * FROM t_general WHERE name = 'hello'");
    assert_mysql_clean("SELECT * FROM t_general WHERE name LIKE 'hel%'");
    assert_mysql_clean("SELECT * FROM t_general WHERE name IN ('a', 'b', 'c')");
    assert_mysql_clean("SELECT COALESCE(name, 'default_val') FROM t_general");
    assert_mysql_clean("SELECT CASE WHEN id = 1 THEN name ELSE 'other' END FROM t_general");
}

#[test]
fn test_explicit_collate_is_silent() {
    assert_mysql_clean(
        "SELECT * FROM t_general g JOIN t_unicode u ON g.name = u.name COLLATE utf8mb4_general_ci",
    );
    assert_mysql_clean(
        "SELECT * FROM t_general g, t_unicode u WHERE g.name COLLATE utf8mb4_unicode_ci = u.name",
    );
    assert_mysql_clean(
        "SELECT * FROM t_general g, t_unicode u WHERE (g.name COLLATE utf8mb4_bin) = u.name",
    );
}

#[test]
fn test_unknown_collation_is_silent() {
    assert_mysql_clean("SELECT * FROM t_general WHERE name = no_coll");
    assert_mysql_clean("SELECT * FROM t_general g JOIN t_unicode u ON g.no_coll = u.name");
    assert_mysql_clean("SELECT * FROM t_general g JOIN t_unicode u ON g.id = u.id");
}

#[test]
fn test_identical_collations_are_silent() {
    assert_mysql_clean("SELECT * FROM t_general WHERE name = tag");
}

#[test]
fn test_other_dialects_are_silent() {
    let pg_sql = "SELECT * FROM t_pg1 p1 JOIN t_pg2 p2 ON p1.name = p2.name";
    let found = pg_codes(pg_sql);
    assert!(
        !found.contains(&"E0036"),
        "expected PostgreSQL to be silent for collation differences, got {found:?}"
    );
}
