//! `Analyzer::describe`: output columns and types of each query

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::{Catalog, QueryDescription, QueryKind, SqlDialect, Templating};

const SCHEMA: &str = "
CREATE TABLE users (id BIGINT PRIMARY KEY, email TEXT NOT NULL, name VARCHAR(100));
CREATE TABLE orders (id BIGINT PRIMARY KEY, user_id BIGINT REFERENCES users(id), total NUMERIC(10,2));
";

fn catalog(dialect: SqlDialect, schema: &str) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    builder.build().0
}

fn describe(sql: &str) -> Vec<QueryDescription> {
    describe_in(SqlDialect::PostgreSQL, sql)
}

fn describe_in(dialect: SqlDialect, sql: &str) -> Vec<QueryDescription> {
    let catalog = catalog(dialect, SCHEMA);
    Analyzer::with_dialect(&catalog, dialect)
        .describe(sql)
        .queries
}

/// (name, type) of each output column, `None` if the columns are unknown
fn columns(query: &QueryDescription) -> Option<Vec<(Option<&str>, Option<String>)>> {
    query.columns.as_ref().map(|columns| {
        columns
            .iter()
            .map(|c| {
                (
                    c.name.as_deref(),
                    c.ty.as_ref().map(sqlsift_core::SqlType::display_name),
                )
            })
            .collect()
    })
}

fn col<'a>(name: &'a str, ty: &str) -> (Option<&'a str>, Option<String>) {
    (Some(name), Some(ty.to_string()))
}

#[test]
fn select_columns_and_types() {
    let queries = describe(
        "SELECT u.id, u.email, count(o.id) AS order_count
         FROM users u LEFT JOIN orders o ON o.user_id = u.id
         GROUP BY u.id;",
    );
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].kind, QueryKind::Select);
    assert_eq!((queries[0].line, queries[0].column), (1, 1));
    assert_eq!(
        columns(&queries[0]).unwrap(),
        vec![
            col("id", "bigint"),
            col("email", "text"),
            col("order_count", "bigint"),
        ]
    );
}

#[test]
fn sqlc_query_names_and_locations() {
    let sql = "-- name: GetUser :one
SELECT id, name FROM users WHERE id = $1;

-- name: ListOrders :many
SELECT * FROM orders;
";
    let queries = describe(sql);
    assert_eq!(queries.len(), 2);
    assert_eq!(queries[0].name.as_deref(), Some("GetUser"));
    assert_eq!(queries[0].line, 2);
    assert_eq!(queries[1].name.as_deref(), Some("ListOrders"));
    assert_eq!(queries[1].line, 5);
    assert_eq!(
        columns(&queries[1]).unwrap(),
        vec![
            col("id", "bigint"),
            col("user_id", "bigint"),
            col("total", "numeric(10,2)"),
        ]
    );
}

#[test]
fn locations_when_a_statement_does_not_parse() {
    let sql = "SELECT id FROM users;\nSELEC oops;\n  SELECT email FROM users;";
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let description = Analyzer::new(&catalog).describe(sql);
    assert_eq!(description.diagnostics.len(), 1);
    let queries = description.queries;
    assert_eq!(queries.len(), 2);
    assert_eq!((queries[1].line, queries[1].column), (3, 3));
}

#[test]
fn dml_returns_its_returning_list_or_nothing() {
    let queries = describe(
        "INSERT INTO users (id, email) VALUES (1, 'a') RETURNING id, email AS mail;
         UPDATE users SET name = 'x' WHERE id = 1;
         DELETE FROM orders WHERE id = 1 RETURNING total;",
    );
    let kinds: Vec<QueryKind> = queries.iter().map(|q| q.kind).collect();
    assert_eq!(
        kinds,
        vec![QueryKind::Insert, QueryKind::Update, QueryKind::Delete]
    );
    assert_eq!(
        columns(&queries[0]).unwrap(),
        vec![col("id", "bigint"), col("mail", "text")]
    );
    assert_eq!(columns(&queries[1]).unwrap(), vec![]);
    assert_eq!(
        columns(&queries[2]).unwrap(),
        vec![col("total", "numeric(10,2)")]
    );
    assert_eq!((queries[1].line, queries[1].column), (2, 10));
}

#[test]
fn ddl_and_other_statements_are_not_described() {
    let queries = describe(
        "CREATE TEMP TABLE t (a INT);
         SET search_path TO public;
         SELECT a FROM t;",
    );
    assert_eq!(queries.len(), 1);
    assert_eq!(columns(&queries[0]).unwrap(), vec![col("a", "integer")]);
}

#[test]
fn unknown_columns_are_null_not_guessed() {
    let queries = describe("SELECT * FROM missing; SELECT nope, id FROM users;");
    assert_eq!(columns(&queries[0]), None);
    assert_eq!(
        columns(&queries[1]).unwrap(),
        vec![(Some("nope"), None), col("id", "bigint")]
    );
}

#[test]
fn postgres_names_of_unaliased_expressions() {
    let queries =
        describe("SELECT ID, Email AS Mail, \"Name\", upper(name), id + 1, 1 FROM users;");
    let names: Vec<Option<&str>> = queries[0]
        .columns
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.name.as_deref())
        .collect();
    // Unquoted names fold to lowercase; `id + 1` and `1` are `?column?`, which
    // isn't reported
    assert_eq!(
        names,
        vec![
            Some("id"),
            Some("mail"),
            Some("Name"),
            Some("upper"),
            None,
            None
        ]
    );
}

#[test]
fn mysql_names_of_unaliased_expressions_are_unknown() {
    let catalog = catalog(
        SqlDialect::MySQL,
        "CREATE TABLE users (id BIGINT PRIMARY KEY, name VARCHAR(100));",
    );
    let queries = Analyzer::with_dialect(&catalog, SqlDialect::MySQL)
        .describe("SELECT ID, upper(name), name AS N FROM users;")
        .queries;
    let names: Vec<Option<&str>> = queries[0]
        .columns
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.name.as_deref())
        .collect();
    assert_eq!(names, vec![Some("ID"), None, Some("N")]);
}

#[test]
fn ctes_and_set_operations() {
    let queries = describe(
        "WITH big AS (SELECT id, total FROM orders WHERE total > 100)
         SELECT b.id, b.total FROM big b
         UNION ALL
         SELECT id, total FROM orders;",
    );
    assert_eq!(
        columns(&queries[0]).unwrap(),
        vec![col("id", "bigint"), col("total", "numeric(10,2)")]
    );
}

#[test]
fn with_insert_is_an_insert() {
    let queries = describe(
        "WITH src AS (SELECT 2 AS id) INSERT INTO users (id, email) SELECT id, 'b' FROM src RETURNING id;",
    );
    assert_eq!(queries[0].kind, QueryKind::Insert);
    assert_eq!(columns(&queries[0]).unwrap(), vec![col("id", "bigint")]);
}

#[test]
fn templated_statements_have_unknown_columns() {
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let queries = Analyzer::new(&catalog)
        .with_templating(Templating::Jinja)
        .describe("select id, {{ extra_columns() }} from users")
        .queries;
    assert_eq!(queries.len(), 1);
    assert_eq!(columns(&queries[0]), None);
}

#[test]
fn embedded_queries_point_into_the_host_file() {
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let source = "const a = 1;\nconst rows = await sql`SELECT id FROM users WHERE id = ${id}`;";
    let queries = Analyzer::new(&catalog)
        .describe_embedded(source, &["sql"])
        .queries;
    assert_eq!(queries.len(), 1);
    assert_eq!((queries[0].line, queries[0].column), (2, 24));
    assert_eq!(columns(&queries[0]).unwrap(), vec![col("id", "bigint")]);
}

#[test]
fn describe_and_analyze_report_the_same_diagnostics() {
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let sql = "SELECT nope FROM users; SELECT 1 FROM missing;";
    let analyzed = Analyzer::new(&catalog).analyze(sql);
    let described = Analyzer::new(&catalog).describe(sql).diagnostics;
    assert_eq!(analyzed.len(), 2);
    assert_eq!(
        analyzed.iter().map(|d| d.kind).collect::<Vec<_>>(),
        described.iter().map(|d| d.kind).collect::<Vec<_>>()
    );
}

#[test]
fn analyze_after_describe_collects_nothing() {
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let mut analyzer = Analyzer::new(&catalog);
    assert_eq!(analyzer.describe("SELECT id FROM users").queries.len(), 1);
    assert!(analyzer.analyze("SELECT id FROM users").is_empty());
    assert_eq!(analyzer.describe("SELECT 1; SELECT 2").queries.len(), 2);
}

#[test]
fn mysql_describes_unknown_dialect_names() {
    let queries = describe_in(SqlDialect::MySQL, "SELECT id, email FROM users");
    assert_eq!(
        columns(&queries[0]).unwrap(),
        vec![col("id", "bigint"), col("email", "text")]
    );
}
