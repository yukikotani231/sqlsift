//! Schema-qualified names: tables, views and column references written as
//! `schema.table`, `database.schema.table`, `schema.table.column` and
//! `database.schema.table.column`, the PostgreSQL search path, and MySQL / SQLite
//! databases acting as schemas.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};

fn catalog(dialect: SqlDialect, schema: &str) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).expect("schema parses");
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "schema warnings: {warnings:?}");
    catalog
}

fn analyze(catalog: &Catalog, dialect: SqlDialect, sql: &str) -> Vec<Diagnostic> {
    Analyzer::with_dialect(catalog, dialect).analyze(sql)
}

#[track_caller]
fn assert_clean(catalog: &Catalog, dialect: SqlDialect, sql: &str) {
    let diags = analyze(catalog, dialect, sql);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for {sql:?}, got: {diags:#?}"
    );
}

#[track_caller]
fn single(catalog: &Catalog, dialect: SqlDialect, sql: &str) -> Diagnostic {
    let mut diags = analyze(catalog, dialect, sql);
    assert_eq!(
        diags.len(),
        1,
        "expected one diagnostic for {sql:?}, got: {diags:#?}"
    );
    diags.remove(0)
}

// =====================================================================
// PostgreSQL
// =====================================================================

const PG: SqlDialect = SqlDialect::PostgreSQL;

/// Supabase-style schema: `auth.users` next to `public.users` and `public.profiles`
const SUPABASE: &str = r"
    CREATE SCHEMA auth;
    CREATE SCHEMA analytics;
    CREATE TABLE auth.users (id uuid PRIMARY KEY, email text NOT NULL, last_sign_in_at timestamptz);
    CREATE TABLE public.users (id bigint PRIMARY KEY, auth_id uuid, display_name text);
    CREATE TABLE profiles (id bigint PRIMARY KEY, user_id bigint, bio text);
    CREATE TABLE analytics.daily_signups (day date, signups integer);
    CREATE VIEW analytics.active_users AS SELECT id, email FROM auth.users;
";

fn pg() -> Catalog {
    catalog(PG, SUPABASE)
}

#[test]
fn catalog_keeps_the_schema_of_tables_and_views() {
    let c = pg();
    let auth = c
        .get_table(&QualifiedName::with_schema("auth", "users"))
        .expect("auth.users");
    assert!(auth.columns.contains_key("email"));
    let public = c
        .get_table(&QualifiedName::with_schema("public", "users"))
        .expect("public.users");
    assert!(public.columns.contains_key("display_name"));
    // Unqualified: the search path (public)
    let unqualified = c.get_table(&QualifiedName::new("users")).expect("users");
    assert!(unqualified.columns.contains_key("display_name"));
    assert!(c.view_exists(&QualifiedName::with_schema("analytics", "active_users")));
    assert!(!c.view_exists(&QualifiedName::new("active_users")));
}

#[test]
fn qualified_and_unqualified_names_resolve_to_the_same_table() {
    let c = pg();
    for sql in [
        "SELECT id, display_name FROM users",
        "SELECT id, display_name FROM public.users",
        "SELECT users.display_name FROM public.users",
        "SELECT public.users.display_name FROM users",
        "SELECT public.users.display_name FROM public.users",
        "SELECT p.bio FROM public.profiles p JOIN profiles q ON q.id = p.id",
    ] {
        assert_clean(&c, PG, sql);
    }
}

#[test]
fn same_table_name_in_two_schemas_does_not_collide() {
    let c = pg();
    assert_clean(&c, PG, "SELECT email, last_sign_in_at FROM auth.users");
    assert_clean(&c, PG, "SELECT auth.users.email FROM auth.users");
    assert_clean(
        &c,
        PG,
        "SELECT a.email, u.display_name FROM auth.users a JOIN public.users u ON u.auth_id = a.id",
    );
    // email is a column of auth.users only
    let d = single(&c, PG, "SELECT email FROM public.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    let d = single(&c, PG, "SELECT display_name FROM auth.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(
        d.message,
        "Column 'display_name' not found in table 'auth.users'"
    );
}

#[test]
fn both_same_named_tables_in_one_from_clause() {
    // PostgreSQL accepts two unaliased tables with the same name from different
    // schemas, and tells them apart by their schema-qualified names
    let c = pg();
    assert_clean(
        &c,
        PG,
        "SELECT auth.users.email, public.users.display_name
         FROM auth.users JOIN public.users ON public.users.auth_id = auth.users.id",
    );
    let d = single(
        &c,
        PG,
        "SELECT auth.users.display_name FROM auth.users JOIN public.users ON public.users.auth_id = auth.users.id",
    );
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(
        d.message,
        "Column 'display_name' not found in table 'auth.users'"
    );
}

#[test]
fn schema_qualified_column_reference_must_name_the_table_in_from() {
    let c = pg();
    // `auth.users` is not in FROM: `users` there is public.users
    let d = single(&c, PG, "SELECT auth.users.email FROM users");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        d.message,
        "Table or alias 'auth.users' not found in FROM clause"
    );
}

#[test]
fn unqualified_table_off_the_search_path_points_to_the_qualified_name() {
    let c = pg();
    let d = single(&c, PG, "SELECT * FROM daily_signups");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.message, "Table 'daily_signups' not found");
    let help = d.help.expect("help");
    assert!(
        help.contains("analytics.daily_signups") && help.contains("search_path"),
        "unexpected help: {help}"
    );
    // Views too
    let d = single(&c, PG, "SELECT * FROM active_users");
    assert!(d.help.expect("help").contains("analytics.active_users"));
}

#[test]
fn search_path_reaches_other_schemas() {
    let mut builder = SchemaBuilder::with_dialect(PG);
    builder.parse(SUPABASE).unwrap();
    let (mut c, _) = builder.build();
    c.search_path = vec!["analytics".into(), "public".into()];
    assert_clean(&c, PG, "SELECT day, signups FROM daily_signups");
    assert_clean(&c, PG, "SELECT display_name FROM users");
    assert_clean(&c, PG, "SELECT email FROM active_users");
    // In a query file
    let c = pg();
    assert_clean(
        &c,
        PG,
        "SET search_path TO analytics, public;\nSELECT day FROM daily_signups;",
    );
}

#[test]
fn three_part_table_names() {
    let c = pg();
    for sql in [
        "SELECT email FROM mydb.auth.users",
        "SELECT display_name FROM mydb.public.users",
        "SELECT u.email FROM mydb.auth.users u",
        "SELECT users.email FROM mydb.auth.users",
        "SELECT auth.users.email FROM mydb.auth.users",
        "SELECT mydb.auth.users.email FROM mydb.auth.users",
        "SELECT mydb.auth.users.email FROM auth.users",
        "INSERT INTO mydb.public.profiles (id, user_id) VALUES (1, 2)",
        "UPDATE mydb.public.profiles SET bio = 'x' WHERE id = 1",
        "DELETE FROM mydb.public.profiles WHERE id = 1",
    ] {
        assert_clean(&c, PG, sql);
    }
    let d = single(&c, PG, "SELECT nope FROM mydb.auth.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    let d = single(&c, PG, "SELECT mydb.auth.users.nope FROM mydb.auth.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(d.message, "Column 'nope' not found in table 'auth.users'");
    let d = single(&c, PG, "SELECT * FROM mydb.auth.nope");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
}

#[test]
fn schema_qualified_wildcards() {
    let c = pg();
    assert_clean(&c, PG, "SELECT auth.users.* FROM auth.users");
    assert_clean(&c, PG, "SELECT public.users.* FROM users");
    assert_clean(
        &c,
        PG,
        "SELECT auth.users.*, public.users.* FROM auth.users JOIN public.users ON public.users.auth_id = auth.users.id",
    );
    let d = single(&c, PG, "SELECT auth.users.* FROM public.users");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
}

#[test]
fn types_flow_through_qualified_column_references() {
    let c = pg();
    // signups is an integer: comparing it with a non-numeric string is a mismatch
    let d = single(
        &c,
        PG,
        "SELECT * FROM analytics.daily_signups WHERE analytics.daily_signups.signups = 'many'",
    );
    assert_eq!(d.kind, DiagnosticKind::TypeMismatch);
    let d = single(
        &c,
        PG,
        "SELECT * FROM mydb.analytics.daily_signups WHERE mydb.analytics.daily_signups.signups = 'many'",
    );
    assert_eq!(d.kind, DiagnosticKind::TypeMismatch);
}

#[test]
fn unknown_schema_is_still_reported() {
    let c = pg();
    let d = single(&c, PG, "SELECT * FROM billing.users");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.message, "Table 'billing.users' not found");
}

#[test]
fn quoted_names_with_dots_are_single_identifiers_in_postgres() {
    let c = catalog(PG, r#"CREATE TABLE "a.b" (id int);"#);
    assert_clean(&c, PG, r#"SELECT id FROM "a.b""#);
    assert_clean(&c, PG, r#"SELECT "a.b".id FROM "a.b""#);
}

#[test]
fn backtick_quoted_dotted_name_splits_into_parts() {
    let c = catalog(
        SqlDialect::MySQL,
        "CREATE TABLE shop.orders (id int, total int);",
    );
    for sql in [
        "SELECT id FROM `shop.orders`",
        "SELECT o.total FROM `shop.orders` o",
        "SELECT `shop.orders`.total FROM `shop.orders`",
        "SELECT orders.total FROM `shop.orders`",
    ] {
        assert_clean(&c, SqlDialect::MySQL, sql);
    }
}

#[test]
fn qualified_name_parse_handles_three_parts() {
    let name = QualifiedName::parse("mydb.auth.users");
    assert_eq!(name.schema.as_deref(), Some("auth"));
    assert_eq!(name.name, "users");
}

// =====================================================================
// MySQL: databases act as schemas
// =====================================================================

const MYSQL: SqlDialect = SqlDialect::MySQL;

#[test]
fn mysql_database_qualified_tables() {
    let c = catalog(
        MYSQL,
        "CREATE TABLE shop.orders (id INT PRIMARY KEY, total INT);
         CREATE TABLE crm.customers (id INT PRIMARY KEY, name VARCHAR(50));
         CREATE TABLE crm.orders (id INT PRIMARY KEY, note TEXT);",
    );
    for sql in [
        "SELECT id, total FROM shop.orders",
        "SELECT shop.orders.total FROM shop.orders",
        "SELECT o.total, c.name FROM shop.orders o JOIN crm.customers c ON c.id = o.id",
        "SELECT note FROM crm.orders",
        "INSERT INTO shop.orders (id, total) VALUES (1, 2)",
        // The current database is chosen at run time: unqualified names may be in any
        "SELECT name FROM customers",
    ] {
        assert_clean(&c, MYSQL, sql);
    }
    let d = single(&c, MYSQL, "SELECT note FROM shop.orders");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    let d = single(&c, MYSQL, "SELECT * FROM shop.customerz");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
}

#[test]
fn mysql_unnamed_database_matches_any_database_qualifier() {
    // Schema dumped without database names: the queries name the database
    let c = catalog(MYSQL, "CREATE TABLE orders (id INT, total INT);");
    assert_clean(&c, MYSQL, "SELECT total FROM shop.orders");
    assert_clean(&c, MYSQL, "SELECT shop.orders.total FROM shop.orders");
    assert_clean(&c, MYSQL, "SELECT total FROM orders");
    let d = single(&c, MYSQL, "SELECT nope FROM shop.orders");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn mysql_use_statement_sets_the_database_of_unqualified_tables() {
    let c = catalog(
        MYSQL,
        "USE shop;
         CREATE TABLE orders (id INT, total INT);
         USE crm;
         CREATE TABLE orders (id INT, note TEXT);",
    );
    assert_clean(&c, MYSQL, "SELECT total FROM shop.orders");
    assert_clean(&c, MYSQL, "SELECT note FROM crm.orders");
    let d = single(&c, MYSQL, "SELECT note FROM shop.orders");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    // In a query file
    assert_clean(&c, MYSQL, "USE crm;\nSELECT note FROM orders;");
}

// =====================================================================
// SQLite: main / temp / attached databases
// =====================================================================

const SQLITE: SqlDialect = SqlDialect::SQLite;

#[test]
fn sqlite_main_and_temp_qualifiers() {
    let c = catalog(
        SQLITE,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);",
    );
    for sql in [
        "SELECT name FROM main.users",
        "SELECT main.users.name FROM main.users",
        "SELECT users.name FROM main.users",
        "SELECT name FROM users",
    ] {
        assert_clean(&c, SQLITE, sql);
    }
    let d = single(&c, SQLITE, "SELECT nope FROM main.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn sqlite_attached_database_tables() {
    let c = catalog(
        SQLITE,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
         CREATE TABLE archive.users (id INTEGER PRIMARY KEY, archived_at TEXT);",
    );
    assert_clean(&c, SQLITE, "SELECT archived_at FROM archive.users");
    assert_clean(&c, SQLITE, "SELECT name FROM users");
    let d = single(&c, SQLITE, "SELECT name FROM archive.users");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
}
