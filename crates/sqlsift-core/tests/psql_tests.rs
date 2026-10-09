// psql scripts: backslash meta-commands and `:var` interpolation (PostgreSQL only)
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const PAGILA: &str = include_str!("../../../tests/fixtures/real-world/pagila-schema.sql");

const SCHEMA: &str = r"
    CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        tags TEXT[],
        created_at DATE
    );
    CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, total NUMERIC);
";

fn catalog(schema: &str, dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    builder.build().0
}

fn analyze_with(schema: &str, dialect: SqlDialect, sql: &str) -> Vec<Diagnostic> {
    let catalog = catalog(schema, dialect);
    Analyzer::with_dialect(&catalog, dialect).analyze(sql)
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    analyze_with(SCHEMA, SqlDialect::PostgreSQL, sql)
}

fn assert_valid(sql: &str) {
    let diagnostics = analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics for:\n{sql}\ngot: {diagnostics:#?}"
    );
}

/// The single diagnostic for `sql`, with its kind, line and column
fn single(sql: &str) -> (DiagnosticKind, usize, usize) {
    let diagnostics = analyze(sql);
    assert_eq!(
        diagnostics.len(),
        1,
        "Expected one diagnostic for:\n{sql}\ngot: {diagnostics:#?}"
    );
    let d = &diagnostics[0];
    let span = d.span.expect("diagnostic has a span");
    (d.kind, span.line, span.column)
}

#[test]
fn issue_94_repro() {
    let sql = "\\set start_date '2024-01-01'\n\
               SELECT customer_id, sum(amount) FROM payment WHERE payment_date >= :'start_date' GROUP BY 1;\n";
    let diagnostics = analyze_with(PAGILA, SqlDialect::PostgreSQL, sql);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn meta_command_lines_are_ignored() {
    assert_valid(
        r"\set ON_ERROR_STOP on
\timing on
\pset format csv
  \echo 'listing users' :foo
\c mydb
\connect mydb postgres
\i other.sql
\ir ../relative.sql
\copy users TO 'users.csv' WITH CSV HEADER
\if :is_admin
SELECT id FROM users;
\elif :other
SELECT name FROM users;
\else
SELECT 1;
\endif
\restrict abc123
\unrestrict abc123
",
    );
}

#[test]
fn meta_command_without_trailing_newline() {
    assert_valid("SELECT id FROM users;\n\\echo done");
    assert_valid("\\echo only a meta-command");
}

#[test]
fn query_terminating_meta_commands() {
    // \g, \gx, \gset, \gexec end the query like `;`
    assert_valid(
        "SELECT max(id) AS max_id FROM users \\gset\n\
         SELECT name FROM users WHERE id = :max_id \\gx\n\
         SELECT 'SELECT 1' \\gexec\n\
         SELECT id FROM users\n\
         \\g\n\
         SELECT id FROM orders\n\
         \\g result.txt\n\
         SELECT id FROM users;\n\
         \\g\n",
    );
}

#[test]
fn psql_variables_are_placeholders() {
    assert_valid("SELECT id FROM users WHERE name = :'name' AND id > :min_id LIMIT :n");
    assert_valid("INSERT INTO users (id, name) VALUES (:id, :'name')");
    assert_valid("UPDATE users SET name = :'name' WHERE id = :id");
    assert_valid("SELECT * FROM users WHERE created_at >= :'start'::date");
}

#[test]
fn psql_variables_are_untyped() {
    // A placeholder is compared against an INTEGER column without a type mismatch
    assert_valid("SELECT id FROM users WHERE id = :'id' OR created_at < :'d'");
}

#[test]
fn quoted_identifier_variable_is_not_reported() {
    assert_valid("SELECT :\"col\" FROM users");
    assert_valid("SELECT id FROM :\"table_name\" WHERE id = 1");
    assert_valid("SELECT t.id FROM :\"table_name\" t");
    assert_valid("SELECT id FROM :\"schema\".users");
    assert_valid("SELECT u.:\"col\" FROM users u");
    assert_valid("INSERT INTO users (id, :\"col\") VALUES (1, 'a')");
}

#[test]
fn unquoted_variable_in_table_position() {
    assert_valid("SELECT * FROM :tbl WHERE x = 1");
    assert_valid("SELECT u.id FROM users u JOIN :other o ON o.user_id = u.id");
    assert_valid("UPDATE :tbl SET x = 1");
}

#[test]
fn casts_slices_and_assignments_are_untouched() {
    assert_valid("SELECT id::text, '1'::int FROM users");
    assert_valid("SELECT tags[1:2], tags[id:id + 1], tags[:2] FROM users");
    assert_valid("SELECT ':name', ':''x''', E'\\\\set', $$ :name \\echo $$ FROM users");
    assert_valid("SELECT \"id\" FROM users -- :name \\echo\n/* :x \\g */");
}

#[test]
fn typo_after_meta_command_is_reported_at_original_location() {
    let sql = "\\set id 1\n\\echo hello\nSELECT nmae FROM users WHERE id = :id;\n";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 3, 8));
}

#[test]
fn typo_after_variable_on_same_line_keeps_column() {
    let sql = "SELECT id FROM users WHERE name = :'a_long_variable_name' AND nmae = 'x';";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 1, 63));
    let sql = "SELECT :\"some_col\", nmae FROM users";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 1, 21));
}

#[test]
fn typo_after_gset_is_reported() {
    let sql = "SELECT max(id) AS m FROM users \\gset\nSELECT id FROM userz WHERE id = :m\n";
    assert_eq!(single(sql), (DiagnosticKind::TableNotFound, 2, 16));
}

#[test]
fn parse_error_after_meta_command_has_original_location() {
    let sql = "\\set x 1\nSELECT id FROM users;\n\\echo hi\nSELECT id,, name FROM users;\n";
    assert_eq!(single(sql), (DiagnosticKind::ParseError, 4, 11));
}

#[test]
fn inline_directives_still_apply() {
    let sql = "\\set x 1\n-- sqlsift:disable E0002\nSELECT nmae FROM users WHERE id = :x;\n";
    assert_valid(sql);
}

#[test]
fn mysql_and_sqlite_are_unaffected() {
    let schema = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);";
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        let diagnostics = analyze_with(schema, dialect, "\\set x 1\nSELECT id FROM users;\n");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::ParseError),
            "{dialect:?}: {diagnostics:#?}"
        );
    }
}

#[test]
fn schema_with_pg_dump_meta_commands() {
    let schema = r"\restrict 4fNyJtSKHvHLBBfmgV3hZZ
--
-- PostgreSQL database dump
--
\connect shop
SET statement_timeout = 0;
CREATE TABLE public.users (id integer NOT NULL, name text);
\unrestrict 4fNyJtSKHvHLBBfmgV3hZZ
CREATE TABLE public.orders (id integer NOT NULL, user_id integer);
";
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::PostgreSQL);
    builder.parse(schema).unwrap();
    let (catalog, diagnostics) = builder.build();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    let diagnostics = Analyzer::new(&catalog)
        .analyze("SELECT u.name FROM users u JOIN orders o ON o.user_id = u.id");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}
