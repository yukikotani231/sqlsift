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

/// (index, name, type) of each parameter
fn parameters(query: &QueryDescription) -> Vec<(usize, Option<&str>, Option<String>)> {
    query
        .parameters
        .iter()
        .map(|p| {
            (
                p.index,
                p.name.as_deref(),
                p.ty.as_ref().map(sqlsift_core::SqlType::display_name),
            )
        })
        .collect()
}

fn param(index: usize, ty: &str) -> (usize, Option<&'static str>, Option<String>) {
    (index, None, Some(ty.to_string()))
}

#[test]
fn parameter_types_from_comparisons_and_assignments() {
    let queries = describe(
        "SELECT id FROM users WHERE email = $2 AND id > $1 LIMIT $3 OFFSET $4;
         INSERT INTO orders (id, user_id, total) VALUES ($1, $2, $3);
         UPDATE users SET name = $1 WHERE id = $2;",
    );
    assert_eq!(
        parameters(&queries[0]),
        vec![
            param(1, "bigint"),
            param(2, "text"),
            param(3, "bigint"),
            param(4, "bigint"),
        ]
    );
    assert_eq!(
        parameters(&queries[1]),
        vec![
            param(1, "bigint"),
            param(2, "bigint"),
            param(3, "numeric(10,2)"),
        ]
    );
    assert_eq!(
        parameters(&queries[2]),
        vec![param(1, "varchar(100)"), param(2, "bigint")]
    );
}

#[test]
fn parameter_types_from_casts_lists_and_patterns() {
    let queries = describe(
        "SELECT id FROM users
         WHERE id IN ($1, $2) AND name LIKE $3 AND id = ANY($4)
           AND id BETWEEN $5 AND $6 AND $7::date IS NOT NULL;",
    );
    assert_eq!(
        parameters(&queries[0]),
        vec![
            param(1, "bigint"),
            param(2, "bigint"),
            param(3, "text"),
            param(4, "bigint[]"),
            param(5, "bigint"),
            param(6, "bigint"),
            param(7, "date"),
        ]
    );
}

#[test]
fn unknown_and_conflicting_parameter_types_are_null() {
    let queries = describe("SELECT id FROM users WHERE id = $1 OR email = $1 OR $2 IS NULL;");
    assert_eq!(
        parameters(&queries[0]),
        vec![(1, None, None), (2, None, None)]
    );
}

#[test]
fn positional_and_named_parameters() {
    let catalog = catalog(SqlDialect::MySQL, SCHEMA);
    let queries = Analyzer::with_dialect(&catalog, SqlDialect::MySQL)
        .describe("SELECT id FROM users WHERE email = ? AND id = ?;")
        .queries;
    assert_eq!(
        parameters(&queries[0]),
        vec![param(1, "text"), param(2, "bigint")]
    );

    // sqlc named parameters, used twice
    let queries = describe(
        "-- name: Find :many
SELECT id FROM users WHERE id = sqlc.arg(user_id) OR (email = @email AND id <> @user_id);",
    );
    assert_eq!(
        parameters(&queries[0]),
        vec![
            (1, Some("user_id"), Some("bigint".to_string())),
            (2, Some("email"), Some("text".to_string())),
        ]
    );

    // aiosql
    let queries = describe("-- name: get-user\nSELECT name FROM users WHERE id = :id;");
    assert_eq!(
        parameters(&queries[0]),
        vec![(1, Some("id"), Some("bigint".to_string()))]
    );
}

#[test]
fn template_literal_interpolations_are_parameters() {
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let source = "await sql`SELECT id FROM users WHERE id = ${a} AND email = ${b}`;";
    let queries = Analyzer::new(&catalog)
        .describe_embedded(source, &["sql"])
        .queries;
    assert_eq!(
        parameters(&queries[0]),
        vec![param(1, "bigint"), param(2, "text")]
    );
}

#[test]
fn psql_variables_are_named_parameters() {
    let queries = describe("SELECT id FROM users WHERE email = :'email';");
    assert_eq!(
        parameters(&queries[0]),
        vec![(1, Some("email"), Some("text".to_string()))]
    );
}

#[test]
fn numeric_literal_types() {
    let queries = describe("SELECT 1 AS a, 3000000000 AS b, 1.5 AS c, 1e3 AS d;");
    assert_eq!(
        columns(&queries[0]).unwrap(),
        vec![
            col("a", "integer"),
            col("b", "bigint"),
            col("c", "numeric"),
            col("d", "numeric"),
        ]
    );
    // Decimal literals still compare with and go into integer columns
    let catalog = catalog(SqlDialect::PostgreSQL, SCHEMA);
    let diagnostics = Analyzer::new(&catalog).analyze(
        "SELECT id FROM users WHERE id > 1.5;
         INSERT INTO orders (id, total) VALUES (2.0, 30000);",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

/// (name, nullable) of each output column
fn nullability(query: &QueryDescription) -> Vec<(&str, Option<bool>)> {
    query
        .columns
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.name.as_deref().unwrap_or("?"), c.nullable))
        .collect()
}

#[test]
fn nullability_of_table_columns_and_outer_joins() {
    let queries = describe(
        "SELECT u.id, u.email, u.name, o.id AS order_id, o.total
         FROM users u LEFT JOIN orders o ON o.user_id = u.id;
         SELECT u.id, o.id AS order_id FROM users u RIGHT JOIN orders o ON o.user_id = u.id;
         SELECT u.id, o.id AS order_id FROM users u FULL JOIN orders o ON o.user_id = u.id;
         SELECT u.id, o.id AS order_id FROM users u JOIN orders o ON o.user_id = u.id;",
    );
    assert_eq!(
        nullability(&queries[0]),
        vec![
            ("id", Some(false)),
            ("email", Some(false)),
            ("name", Some(true)),
            ("order_id", Some(true)),
            ("total", Some(true)),
        ]
    );
    assert_eq!(
        nullability(&queries[1]),
        vec![("id", Some(true)), ("order_id", Some(false))]
    );
    assert_eq!(
        nullability(&queries[2]),
        vec![("id", Some(true)), ("order_id", Some(true))]
    );
    assert_eq!(
        nullability(&queries[3]),
        vec![("id", Some(false)), ("order_id", Some(false))]
    );
}

#[test]
fn nullability_of_expressions() {
    let queries = describe(
        "SELECT count(*) AS n, sum(o.total) AS s, max(o.id) AS m,
                coalesce(o.total, 0) AS c, o.id + 1 AS plus, o.total + 1 AS plus_null,
                o.total IS NULL AS missing, upper(u.email) AS up, lower(u.name) AS low,
                CASE WHEN o.id > 1 THEN 'a' END AS no_else,
                CASE WHEN o.id > 1 THEN 'a' ELSE 'b' END AS with_else,
                (SELECT 1) AS sub, NULL AS nothing, 'x' AS lit, $1 AS param,
                some_function(o.id) AS unknown
         FROM orders o JOIN users u ON u.id = o.user_id
         GROUP BY o.id, u.email, u.name, o.total;",
    );
    assert_eq!(
        nullability(&queries[0]),
        vec![
            ("n", Some(false)),
            ("s", Some(true)),
            ("m", Some(true)),
            ("c", Some(false)),
            ("plus", Some(false)),
            ("plus_null", Some(true)),
            ("missing", Some(false)),
            ("up", Some(false)),
            ("low", Some(true)),
            ("no_else", Some(true)),
            ("with_else", Some(false)),
            ("sub", Some(true)),
            ("nothing", Some(true)),
            ("lit", Some(false)),
            ("param", None),
            ("unknown", None),
        ]
    );
}

#[test]
fn nullability_through_ctes_subqueries_and_set_operations() {
    let queries = describe(
        "WITH x AS (SELECT u.id, o.id AS oid FROM users u LEFT JOIN orders o ON o.user_id = u.id)
         SELECT x.id, x.oid, d.email FROM x JOIN (SELECT id, email FROM users) d ON d.id = x.id;
         SELECT id, email FROM users UNION ALL SELECT id, NULL FROM orders;
         SELECT * FROM users;",
    );
    assert_eq!(
        nullability(&queries[0]),
        vec![
            ("id", Some(false)),
            ("oid", Some(true)),
            ("email", Some(false))
        ]
    );
    assert_eq!(
        nullability(&queries[1]),
        vec![("id", Some(false)), ("email", Some(true))]
    );
    assert_eq!(
        nullability(&queries[2]),
        vec![
            ("id", Some(false)),
            ("email", Some(false)),
            ("name", Some(true))
        ]
    );
}

#[test]
fn rollup_makes_columns_nullable() {
    let queries = describe("SELECT user_id, count(*) AS n FROM orders GROUP BY ROLLUP (user_id);");
    assert_eq!(
        nullability(&queries[0]),
        vec![("user_id", Some(true)), ("n", Some(true))]
    );
}

#[test]
fn sqlite_text_primary_keys_may_be_null() {
    let catalog = catalog(
        SqlDialect::SQLite,
        "CREATE TABLE t (code TEXT PRIMARY KEY, n INTEGER PRIMARY KEY);",
    );
    let queries = Analyzer::with_dialect(&catalog, SqlDialect::SQLite)
        .describe("SELECT code FROM t;")
        .queries;
    assert_eq!(nullability(&queries[0]), vec![("code", Some(true))]);

    let catalog = catalog_sqlite_int();
    let queries = Analyzer::with_dialect(&catalog, SqlDialect::SQLite)
        .describe("SELECT id FROM t;")
        .queries;
    assert_eq!(nullability(&queries[0]), vec![("id", Some(false))]);
}

fn catalog_sqlite_int() -> Catalog {
    catalog(
        SqlDialect::SQLite,
        "CREATE TABLE t (id INTEGER PRIMARY KEY);",
    )
}

#[test]
fn mysql_division_may_be_null() {
    let queries = describe_in(
        SqlDialect::MySQL,
        "SELECT id / 2 AS half, id * 2 AS twice FROM users;",
    );
    assert_eq!(
        nullability(&queries[0]),
        vec![("half", Some(true)), ("twice", Some(false))]
    );
}

#[test]
fn returning_nullability() {
    let queries = describe("INSERT INTO users (id, email) VALUES (1, 'a') RETURNING id, name;");
    assert_eq!(
        nullability(&queries[0]),
        vec![("id", Some(false)), ("name", Some(true))]
    );
}
