// sqlc query files: named parameters (`@name`, `sqlc.arg(name)`, `sqlc.narg(name)`,
// `sqlc.slice(name)`) are untyped placeholders, like `$1`
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::SchemaBuilder;

const SCHEMA: &str = r"
    CREATE TABLE users (id INTEGER NOT NULL, name TEXT, tags TEXT[], created_at DATE);
    CREATE TABLE posts (id INTEGER NOT NULL, author_id INTEGER, status TEXT);
";

const MYSQL_SCHEMA: &str = r"
    CREATE TABLE users (id INT NOT NULL, name VARCHAR(100), created_at DATE);
    CREATE TABLE posts (id INT NOT NULL, author_id INT, status VARCHAR(20));
";

fn analyze_with(dialect: SqlDialect, sql: &str) -> Vec<Diagnostic> {
    let schema = match dialect {
        SqlDialect::PostgreSQL => SCHEMA,
        SqlDialect::MySQL | SqlDialect::SQLite => MYSQL_SCHEMA,
    };
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect).analyze(sql)
}

fn assert_valid_with(dialect: SqlDialect, sql: &str) {
    let diagnostics = analyze_with(dialect, sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics ({dialect:?}) for:\n{sql}\ngot: {diagnostics:#?}"
    );
}

fn assert_valid(sql: &str) {
    assert_valid_with(SqlDialect::PostgreSQL, sql);
}

/// (kind, line, column) of each diagnostic
fn locations(dialect: SqlDialect, sql: &str) -> Vec<(DiagnosticKind, usize, usize)> {
    analyze_with(dialect, sql)
        .iter()
        .map(|d| {
            let span = d.span.expect("diagnostic has a span");
            (d.kind, span.line, span.column)
        })
        .collect()
}

#[test]
fn issue_119_repro() {
    let sql = "-- name: A :many\n\
               SELECT * FROM users WHERE name = sqlc.narg(filter_name);\n\
               -- name: B :many\n\
               SELECT * FROM users WHERE id > @after_id LIMIT @page_size;\n\
               -- name: C :many\n\
               SELECT * FROM users WHERE name = sqlc.arg(nm)::text;\n\
               -- name: D :many\n\
               SELECT status, count(*) FROM posts GROUP BY status HAVING count(*) > sqlc.arg(min_count);\n";
    assert_valid(sql);
}

#[test]
fn sqlc_macros_are_placeholders_in_every_dialect() {
    for dialect in [
        SqlDialect::PostgreSQL,
        SqlDialect::MySQL,
        SqlDialect::SQLite,
    ] {
        for sql in [
            "SELECT * FROM users WHERE name = sqlc.arg(name)",
            "SELECT * FROM users WHERE name = sqlc.arg('name')",
            "SELECT * FROM users WHERE name = sqlc.arg(\"name\")",
            "SELECT * FROM users WHERE name = sqlc.narg(filter_name) OR sqlc.narg(filter_name) IS NULL",
            "SELECT * FROM users WHERE id IN (sqlc.slice(ids))",
            "SELECT * FROM users WHERE id IN (sqlc.slice('ids')) AND name = sqlc.arg( nm )",
            "SELECT * FROM users WHERE name = SQLC.ARG(nm)",
            "INSERT INTO users (id, name) VALUES (sqlc.arg(id), sqlc.narg(name))",
            "UPDATE users SET name = sqlc.arg(name) WHERE id = sqlc.arg(id)",
            "DELETE FROM users WHERE id = sqlc.arg(id)",
            "SELECT * FROM users LIMIT sqlc.arg(lim) OFFSET sqlc.arg(off)",
        ] {
            assert_valid_with(dialect, sql);
        }
    }
}

#[test]
fn sqlc_parameters_are_untyped() {
    // Compared against INTEGER and DATE columns without a type mismatch
    assert_valid("SELECT * FROM users WHERE id = @id AND created_at > @since");
    assert_valid("SELECT * FROM users WHERE id = sqlc.arg(id) AND created_at > sqlc.arg(since)");
    assert_valid("SELECT * FROM users WHERE id = @id::int");
    assert_valid("INSERT INTO users (id, name, created_at) VALUES (@id, @name, @created_at)");
    assert_valid("UPDATE users SET name = @name WHERE id = @id RETURNING id");
}

#[test]
fn at_parameters_in_postgresql() {
    assert_valid("SELECT * FROM users WHERE id > @after_id ORDER BY id LIMIT @page_size");
    assert_valid("SELECT * FROM users WHERE (id=@id)");
    assert_valid("SELECT * FROM users WHERE id = ANY(@ids::int[])");
    assert_valid(
        "SELECT * FROM users u JOIN posts p ON p.author_id = u.id WHERE p.status = @status",
    );
}

#[test]
fn postgresql_at_operators_are_untouched() {
    assert_valid("SELECT * FROM users WHERE tags @> ARRAY['a'] AND ARRAY['b'] <@ tags");
    assert_valid("SELECT * FROM users WHERE tags@>ARRAY['a'] AND ARRAY['b']<@tags");
    assert_valid("SELECT * FROM users WHERE to_tsvector(name) @@ to_tsquery('x')");
    assert_valid("SELECT @ -5, @ id FROM users");
    // A misspelled column after `<@` / `@@` is still reported
    assert_eq!(
        locations(
            SqlDialect::PostgreSQL,
            "SELECT * FROM users WHERE ARRAY['b']<@tagz"
        ),
        vec![(DiagnosticKind::ColumnNotFound, 1, 39)]
    );
}

#[test]
fn at_inside_literals_and_comments_is_untouched() {
    // An email in a string literal stays a string (and `@` in comments is ignored)
    assert_valid("SELECT * FROM users WHERE name = 'a@b.com' -- @nope\n/* @nope sqlc.arg(x) */");
    assert_eq!(
        analyze_with(
            SqlDialect::PostgreSQL,
            "SELECT * FROM users WHERE id = 'x@y'"
        )
        .len(),
        1,
        "a string literal compared with an INTEGER column is still a type mismatch"
    );
    assert_valid("SELECT 'sqlc.arg(x)', $$ @x $$ FROM users");
}

#[test]
fn mysql_user_variables_are_untouched() {
    // `@var` is a user variable in MySQL (and a bind parameter in SQLite); sqlc only
    // supports `sqlc.arg()` there
    assert_valid_with(SqlDialect::MySQL, "SET @x = 1");
    assert_valid_with(
        SqlDialect::MySQL,
        "SELECT * FROM users WHERE id > @after_id",
    );
    assert_valid_with(
        SqlDialect::SQLite,
        "SELECT * FROM users WHERE id > @after_id",
    );
}

#[test]
fn locations_are_kept() {
    // Diagnostics after a masked parameter keep their columns
    assert_eq!(
        locations(
            SqlDialect::PostgreSQL,
            "-- name: X :many\nSELECT nme FROM users WHERE id = sqlc.arg(id) AND nmae = @name"
        ),
        vec![
            (DiagnosticKind::ColumnNotFound, 2, 8),
            (DiagnosticKind::ColumnNotFound, 2, 51),
        ]
    );
    assert_eq!(
        locations(
            SqlDialect::MySQL,
            "SELECT * FROM users WHERE id IN (sqlc.slice(ids)) AND nmae = 1"
        ),
        vec![(DiagnosticKind::ColumnNotFound, 1, 55)]
    );
}

#[test]
fn other_sqlc_lookalikes_are_untouched() {
    // A real column or function that isn't a sqlc macro is still checked
    assert_eq!(
        locations(
            SqlDialect::PostgreSQL,
            "SELECT * FROM users WHERE name = mysqlc.arg(x)"
        )
        .len(),
        1
    );
    assert_eq!(
        locations(SqlDialect::PostgreSQL, "SELECT sqlc.argz(nme) FROM users")
            .iter()
            .filter(|(k, _, _)| *k == DiagnosticKind::ColumnNotFound)
            .count(),
        1
    );
}
