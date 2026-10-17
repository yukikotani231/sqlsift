//! E0037 check-constraint-violation: literals that violate a CHECK constraint in
//! INSERT VALUES or UPDATE SET.

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE users (
    id INT PRIMARY KEY,
    age INT CONSTRAINT users_age_check CHECK (age >= 0),
    status TEXT CONSTRAINT users_status_check CHECK (status IN ('active', 'banned')),
    role TEXT CHECK (role <> 'banned_role')
);

CREATE TABLE ranges (
    id INT PRIMARY KEY,
    lo INT,
    hi INT,
    CONSTRAINT lo_le_hi CHECK (lo <= hi)
);

CREATE TABLE orders (
    id INT PRIMARY KEY,
    qty INT CHECK (qty BETWEEN 1 AND 100),
    discount INT CHECK (discount >= 0 AND discount <= 50),
    category TEXT CHECK (category NOT IN ('archived', 'deleted'))
);
";

fn codes_in(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    codes_with_schema(dialect, SCHEMA, sql)
}

fn codes_with_schema(dialect: SqlDialect, schema: &str, sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[track_caller]
fn assert_reports(sql: &str) {
    assert_eq!(codes_in(SqlDialect::PostgreSQL, sql), ["E0037"], "{sql}");
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
fn single_column_comparison_check() {
    assert_reports("INSERT INTO users (id, age) VALUES (1, -1)");
    assert_clean("INSERT INTO users (id, age) VALUES (1, 0)");
    assert_clean("INSERT INTO users (id, age) VALUES (1, 25)");
    // NULL satisfies a CHECK constraint in SQL
    assert_clean("INSERT INTO users (id, age) VALUES (1, NULL)");
}

#[test]
fn update_set_violates_check() {
    assert_reports("UPDATE users SET status = 'actve' WHERE id = 1");
    assert_clean("UPDATE users SET status = 'active' WHERE id = 1");
    assert_clean("UPDATE users SET status = 'banned' WHERE id = 1");
    assert_clean("UPDATE users SET status = NULL WHERE id = 1");
}

#[test]
fn positional_insert_without_columns() {
    assert_reports("INSERT INTO users VALUES (1, -5, 'active', 'user')");
    assert_clean("INSERT INTO users VALUES (1, 20, 'active', 'user')");
}

#[test]
fn multi_row_insert_reports_violating_row() {
    let diags = codes_in(
        SqlDialect::PostgreSQL,
        "INSERT INTO users (id, age) VALUES (1, 20), (2, -1), (3, 30)",
    );
    assert_eq!(diags, vec!["E0037"]);
}

#[test]
fn multi_column_table_constraint() {
    // Both columns assigned and lo > hi -> violates lo <= hi
    assert_reports("INSERT INTO ranges (id, lo, hi) VALUES (1, 10, 5)");
    assert_clean("INSERT INTO ranges (id, lo, hi) VALUES (1, 5, 10)");
    assert_clean("INSERT INTO ranges (id, lo, hi) VALUES (1, 5, 5)");

    // Only one column assigned: check is unknown, not reported
    assert_clean("INSERT INTO ranges (id, lo) VALUES (1, 10)");
    assert_clean("UPDATE ranges SET lo = 20 WHERE id = 1");

    // Both updated: lo > hi
    assert_reports("UPDATE ranges SET lo = 20, hi = 10 WHERE id = 1");
    assert_clean("UPDATE ranges SET lo = 5, hi = 10 WHERE id = 1");
}

#[test]
fn between_and_in_list_checks() {
    assert_reports("INSERT INTO orders (id, qty) VALUES (1, 0)");
    assert_reports("INSERT INTO orders (id, qty) VALUES (1, 101)");
    assert_clean("INSERT INTO orders (id, qty) VALUES (1, 1)");
    assert_clean("INSERT INTO orders (id, qty) VALUES (1, 50)");
    assert_clean("INSERT INTO orders (id, qty) VALUES (1, 100)");

    assert_reports("INSERT INTO orders (id, discount) VALUES (1, 60)");
    assert_clean("INSERT INTO orders (id, discount) VALUES (1, 25)");

    assert_reports("INSERT INTO orders (id, category) VALUES (1, 'archived')");
    assert_reports("INSERT INTO orders (id, category) VALUES (1, 'deleted')");
    assert_clean("INSERT INTO orders (id, category) VALUES (1, 'electronics')");
}

#[test]
fn string_equality_check() {
    assert_reports("INSERT INTO users (id, role) VALUES (1, 'banned_role')");
    assert_clean("INSERT INTO users (id, role) VALUES (1, 'user')");
}

#[test]
fn drop_constraint_removes_check() {
    let schema = "
    CREATE TABLE accounts (
        id INT PRIMARY KEY,
        balance INT CONSTRAINT acc_balance_check CHECK (balance >= 0)
    );
    ALTER TABLE accounts DROP CONSTRAINT acc_balance_check;
    ";
    let diags = codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO accounts (id, balance) VALUES (1, -50)",
    );
    assert_eq!(diags, Vec::<&str>::new());
}

#[test]
fn create_table_like_constraints_copied_only_with_including_constraints() {
    let schema = "
    CREATE TABLE base_items (
        id INT PRIMARY KEY,
        qty INT CHECK (qty > 0)
    );
    CREATE TABLE copy_plain (LIKE base_items);
    CREATE TABLE copy_with_chk (LIKE base_items INCLUDING CONSTRAINTS);
    ";
    // Plain LIKE in PostgreSQL excludes constraints: no violation
    let diags_plain = codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO copy_plain (id, qty) VALUES (1, -1)",
    );
    assert_eq!(diags_plain, Vec::<&str>::new());

    // LIKE INCLUDING CONSTRAINTS copies check constraints: reports violation
    let diags_with_chk = codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO copy_with_chk (id, qty) VALUES (1, -1)",
    );
    assert_eq!(diags_with_chk, vec!["E0037"]);
}

#[test]
fn not_enforced_check_constraint_is_skipped() {
    let schema = "
    CREATE TABLE items (
        id INT PRIMARY KEY,
        qty INT CHECK (qty > 0) NOT ENFORCED
    );
    ";
    let diags = codes_with_schema(
        SqlDialect::MySQL,
        schema,
        "INSERT INTO items (id, qty) VALUES (1, -1)",
    );
    assert_eq!(diags, Vec::<&str>::new());
}

#[test]
fn mysql_skips_string_comparisons() {
    // In MySQL, default collation is case-insensitive, so string comparisons stay quiet
    let sql = "UPDATE users SET status = 'actve' WHERE id = 1";
    assert_eq!(
        codes_in(SqlDialect::MySQL, sql),
        Vec::<&str>::new(),
        "MySQL string comparisons should stay quiet"
    );
    // In PostgreSQL, string comparisons are checked
    assert_eq!(codes_in(SqlDialect::PostgreSQL, sql), vec!["E0037"]);
}

#[test]
fn literal_converted_to_column_type_edge_cases() {
    let schema = "
    CREATE TABLE edge_cases (
        i INT CHECK (i > 0),
        c TEXT CHECK (c <> '07'),
        n NUMERIC(10, 2) CHECK (n > 0.01),
        d DATE CHECK (d > '2020-01-01')
    );
    ";
    // 2.6 into INT: float is not converted, stays quiet for E0037
    assert!(!codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO edge_cases (i) VALUES (2.6)"
    )
    .contains(&"E0037"));

    // Integer 7 into TEXT: number not converted to string, stays quiet for E0037
    assert!(!codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO edge_cases (c) VALUES (7)"
    )
    .contains(&"E0037"));

    // 0.005 into NUMERIC(10,2): exceeds column scale (rounded by database), stays quiet for E0037
    assert!(!codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO edge_cases (n) VALUES (0.005)"
    )
    .contains(&"E0037"));

    // Exact numeric comparison within scale: violates n > 0.01
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            schema,
            "INSERT INTO edge_cases (n) VALUES (0.00)"
        ),
        vec!["E0037"]
    );
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            schema,
            "INSERT INTO edge_cases (n) VALUES (0.05)"
        ),
        Vec::<&str>::new()
    );

    // DATE column: not plain integer, numeric or text, stays quiet for E0037
    assert!(!codes_with_schema(
        SqlDialect::PostgreSQL,
        schema,
        "INSERT INTO edge_cases (d) VALUES ('2019-01-01')"
    )
    .contains(&"E0037"));
}

#[test]
fn dialect_enforcement() {
    let sql = "INSERT INTO users (id, age) VALUES (1, -1)";

    // PostgreSQL, MySQL, and SQLite enforce integer CHECK constraints
    assert_eq!(codes_in(SqlDialect::PostgreSQL, sql), vec!["E0037"]);
    assert_eq!(codes_in(SqlDialect::MySQL, sql), vec!["E0037"]);
    assert_eq!(codes_in(SqlDialect::SQLite, sql), vec!["E0037"]);

    // Warehouses treat CHECK constraints as informational / unenforced
    assert_eq!(codes_in(SqlDialect::Snowflake, sql), Vec::<&str>::new());
    assert_eq!(codes_in(SqlDialect::Redshift, sql), Vec::<&str>::new());
}

#[test]
fn diagnostic_message_and_help() {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::PostgreSQL);
    builder.parse(SCHEMA).expect("schema parses");
    let catalog = builder.build().0;
    let diags = Analyzer::with_dialect(&catalog, SqlDialect::PostgreSQL)
        .analyze("INSERT INTO users (id, age) VALUES (1, -1)");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code(), "E0037");
    assert!(
        diags[0].message.contains("users_age_check"),
        "expected constraint name in message: {}",
        diags[0].message
    );
    assert!(
        diags[0].help.as_ref().unwrap().contains("age >= 0"),
        "expected expression in help: {:?}",
        diags[0].help
    );
}

#[test]
fn skips_columns_with_explicit_collation() {
    let pg_schema = r#"
    CREATE TABLE t (
        c TEXT COLLATE "und-x-icu" CHECK (c IN ('a', 'b'))
    );
    "#;
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_schema,
            "INSERT INTO t (c) VALUES ('A')"
        ),
        Vec::<&str>::new()
    );

    let sqlite_schema = "
    CREATE TABLE t (
        c1 TEXT COLLATE NOCASE CHECK (c1 IN ('a', 'b')),
        c2 TEXT COLLATE RTRIM CHECK (c2 = 'ab')
    );
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::SQLite,
            sqlite_schema,
            "INSERT INTO t (c1, c2) VALUES ('A', 'ab   ')"
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn unnamed_check_dropped_by_generated_name() {
    // PostgreSQL column-level unnamed CHECK: t_x_check
    let pg_col_schema = "
    CREATE TABLE t (x INT CHECK (x > 0));
    ALTER TABLE t DROP CONSTRAINT t_x_check;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_col_schema,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );

    // PostgreSQL table-level unnamed CHECK: t_check
    let pg_tbl_schema = "
    CREATE TABLE t (x INT, y INT, CHECK (x < y));
    ALTER TABLE t DROP CONSTRAINT t_check;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_tbl_schema,
            "INSERT INTO t (x, y) VALUES (10, 5)"
        ),
        Vec::<&str>::new()
    );

    // PostgreSQL single-column table-level unnamed CHECK dropped as t_check
    let pg_single_tbl_schema = "
    CREATE TABLE t (x INT, CHECK (x > 0));
    ALTER TABLE t DROP CONSTRAINT t_check;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_single_tbl_schema,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );

    // MySQL unnamed CHECK: t_chk_1
    let mysql_schema = "
    CREATE TABLE t (x INT CHECK (x > 0));
    ALTER TABLE t DROP CHECK t_chk_1;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::MySQL,
            mysql_schema,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn other_ways_check_constraint_removed_or_disabled() {
    // MySQL DROP CHECK k
    let mysql_drop = "
    CREATE TABLE t (x INT CONSTRAINT k CHECK (x > 0));
    ALTER TABLE t DROP CHECK k;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::MySQL,
            mysql_drop,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );

    // MySQL ALTER CHECK k NOT ENFORCED
    let mysql_not_enforced = "
    CREATE TABLE t (x INT CONSTRAINT k CHECK (x > 0));
    ALTER TABLE t ALTER CHECK k NOT ENFORCED;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::MySQL,
            mysql_not_enforced,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );

    // PostgreSQL RENAME CONSTRAINT followed by DROP CONSTRAINT
    let pg_rename_drop = "
    CREATE TABLE t (x INT CHECK (x > 0));
    ALTER TABLE t RENAME CONSTRAINT t_x_check TO positive_x;
    ALTER TABLE t DROP CONSTRAINT positive_x;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_rename_drop,
            "INSERT INTO t (x) VALUES (-1)"
        ),
        Vec::<&str>::new()
    );

    // Dropping and re-adding the column drops the CHECK constraint with it
    let pg_drop_readd_col = "
    CREATE TABLE t (id INT, x INT CHECK (x > 0));
    ALTER TABLE t DROP COLUMN x;
    ALTER TABLE t ADD COLUMN x INT;
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            pg_drop_readd_col,
            "INSERT INTO t (id, x) VALUES (1, -1)"
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn postgres_varchar_trailing_spaces_truncated_before_check() {
    let schema = "
    CREATE TABLE t (
        c VARCHAR(2) CHECK (c = 'ab')
    );
    ";
    assert_eq!(
        codes_with_schema(
            SqlDialect::PostgreSQL,
            schema,
            "INSERT INTO t (c) VALUES ('ab   ')"
        ),
        Vec::<&str>::new()
    );
}

#[test]
fn sqlite_skips_non_integer_arithmetic_in_check() {
    let schema = "
    CREATE TABLE t (
        x INT CHECK (x * 0.1 <> 0.3)
    );
    ";
    assert_eq!(
        codes_with_schema(SqlDialect::SQLite, schema, "INSERT INTO t (x) VALUES (3)"),
        Vec::<&str>::new()
    );
}
