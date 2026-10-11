//! Issue #138: hints on renamed and dropped tables and columns name the schema
//! file and line of the ALTER TABLE / DROP that made the change, or the line of
//! the query file itself.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

/// Build a catalog from `(path, sql)` schema files, in order
fn catalog(files: &[(&str, &str)], dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    for (path, sql) in files {
        builder.parse_file(sql, path).expect("schema parses");
    }
    builder.build().0
}

fn pg(files: &[(&str, &str)]) -> Catalog {
    catalog(files, SqlDialect::PostgreSQL)
}

#[track_caller]
fn single(catalog: &Catalog, dialect: SqlDialect, sql: &str) -> Diagnostic {
    let mut diags = Analyzer::with_dialect(catalog, dialect).analyze(sql);
    assert_eq!(diags.len(), 1, "for {sql:?}: {diags:#?}");
    diags.remove(0)
}

#[track_caller]
fn help(catalog: &Catalog, sql: &str) -> String {
    single(catalog, SqlDialect::PostgreSQL, sql)
        .help
        .unwrap_or_default()
}

const CREATE: (&str, &str) = (
    "db/migrations/0001_create.sql",
    "CREATE TABLE posts (id INT PRIMARY KEY, body TEXT, nick TEXT);
CREATE TABLE people (id INT PRIMARY KEY, name TEXT);
CREATE TABLE old_logs (id INT);
",
);

#[test]
fn renamed_column_names_the_migration() {
    let c = pg(&[
        CREATE,
        (
            "db/migrations/0002_rename_body.sql",
            "-- Posts have rich content now\n\nALTER TABLE posts\n  RENAME COLUMN body TO content;\n",
        ),
    ]);
    let d = single(&c, SqlDialect::PostgreSQL, "SELECT body FROM posts");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was renamed to 'content' in db/migrations/0002_rename_body.sql:4")
    );
}

#[test]
fn dropped_column_names_the_migration() {
    let c = pg(&[
        CREATE,
        (
            "db/migrations/0002_drop_nick.sql",
            "ALTER TABLE posts DROP COLUMN nick;",
        ),
    ]);
    assert_eq!(
        help(&c, "SELECT nick FROM posts"),
        "'nick' was dropped in db/migrations/0002_drop_nick.sql:1"
    );
}

#[test]
fn chained_renames_name_the_latest_migration() {
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "ALTER TABLE posts RENAME COLUMN body TO text_body;",
        ),
        (
            "m/0003.sql",
            "\nALTER TABLE posts RENAME COLUMN text_body TO content;",
        ),
    ]);
    assert_eq!(
        help(&c, "SELECT body FROM posts"),
        "'body' was renamed to 'content' in m/0003.sql:2"
    );
    // Renamed, then dropped under the new name
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "ALTER TABLE posts RENAME COLUMN body TO content;",
        ),
        ("m/0003.sql", "ALTER TABLE posts DROP COLUMN content;"),
    ]);
    assert_eq!(
        help(&c, "SELECT body FROM posts"),
        "'body' was dropped in m/0003.sql:1"
    );
}

#[test]
fn renamed_table_says_what_it_was_renamed_to() {
    let c = pg(&[
        CREATE,
        (
            "db/migrations/0002_rename_people.sql",
            "\nALTER TABLE people RENAME TO persons;",
        ),
    ]);
    let d = single(&c, SqlDialect::PostgreSQL, "SELECT name FROM people");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        d.help.as_deref(),
        Some("'people' was renamed to 'persons' in db/migrations/0002_rename_people.sql:2")
    );
    // Qualified as written
    assert_eq!(
        help(&c, "SELECT name FROM public.people"),
        "'public.people' was renamed to 'public.persons' in db/migrations/0002_rename_people.sql:2"
    );
    assert!(Analyzer::new(&c)
        .analyze("SELECT name FROM persons")
        .is_empty());
}

#[test]
fn table_renames_are_followed_and_reset() {
    let c = pg(&[
        CREATE,
        ("m/0002.sql", "ALTER TABLE people RENAME TO persons;"),
        ("m/0003.sql", "ALTER TABLE persons RENAME TO users;"),
    ]);
    assert_eq!(
        help(&c, "SELECT name FROM people"),
        "'people' was renamed to 'users' in m/0003.sql:1"
    );
    assert_eq!(
        help(&c, "SELECT name FROM persons"),
        "'persons' was renamed to 'users' in m/0003.sql:1"
    );

    // A table created again under the old name
    let c = pg(&[
        CREATE,
        ("m/0002.sql", "ALTER TABLE people RENAME TO persons;"),
        ("m/0003.sql", "CREATE TABLE people (id INT, nickname TEXT);"),
    ]);
    assert!(Analyzer::new(&c)
        .analyze("SELECT nickname FROM people")
        .is_empty());
}

#[test]
fn dropped_table_names_the_migration() {
    let c = pg(&[
        CREATE,
        (
            "db/migrations/0002_drop_logs.sql",
            "DROP TABLE IF EXISTS no_such_table;\nDROP TABLE old_logs;",
        ),
    ]);
    let d = single(&c, SqlDialect::PostgreSQL, "SELECT id FROM old_logs");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        d.help.as_deref(),
        Some("'old_logs' was dropped in db/migrations/0002_drop_logs.sql:2")
    );
    // A table that never existed gets no such hint
    let help = help(&c, "SELECT id FROM no_such_table");
    assert!(!help.contains("dropped"), "{help}");

    // Dropped and created again
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "DROP TABLE old_logs; CREATE TABLE old_logs (id INT, at TIMESTAMP);",
        ),
    ]);
    assert!(Analyzer::new(&c)
        .analyze("SELECT at FROM old_logs")
        .is_empty());
}

#[test]
fn dropped_view_names_the_migration() {
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "CREATE VIEW post_ids AS SELECT id FROM posts;",
        ),
        ("m/0003.sql", "DROP VIEW post_ids;"),
    ]);
    assert_eq!(
        help(&c, "SELECT id FROM post_ids"),
        "'post_ids' was dropped in m/0003.sql:1"
    );
}

#[test]
fn schema_without_file_names_keeps_the_generic_wording() {
    let mut builder = SchemaBuilder::new();
    builder
        .parse(
            "CREATE TABLE people (id INT, name TEXT);
             ALTER TABLE people RENAME COLUMN name TO full_name;
             ALTER TABLE people RENAME TO persons;
             CREATE TABLE old_logs (id INT);
             DROP TABLE old_logs;",
        )
        .unwrap();
    let c = builder.build().0;
    assert_eq!(
        help(&c, "SELECT id FROM people"),
        "'people' was renamed to 'persons' by ALTER TABLE in the schema"
    );
    assert_eq!(
        help(&c, "SELECT id FROM old_logs"),
        "'old_logs' was dropped in the schema"
    );
    assert_eq!(
        help(&c, "SELECT name FROM persons"),
        "'name' was renamed to 'full_name' by ALTER TABLE in the schema"
    );
}

#[test]
fn line_is_right_when_statements_are_parsed_one_by_one() {
    // The function body doesn't parse, so the file is parsed statement by statement
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "CREATE FUNCTION f() RETURNS trigger AS $$ BEGIN RETURN NEW; END $$ LANGUAGE plpgsql;
CREATE TRIGGER t BEFORE UPDATE ON posts FOR EACH ROW EXECUTE FUNCTION f(;

ALTER TABLE posts RENAME COLUMN body TO content; ALTER TABLE posts
  DROP COLUMN nick;
",
        ),
    ]);
    assert_eq!(
        help(&c, "SELECT body FROM posts"),
        "'body' was renamed to 'content' in m/0002.sql:4"
    );
    assert_eq!(
        help(&c, "SELECT nick FROM posts"),
        "'nick' was dropped in m/0002.sql:5"
    );
}

#[test]
fn dbmate_down_section_keeps_lines() {
    let c = pg(&[
        CREATE,
        (
            "db/migrations/20240312_rename_body.sql",
            "-- migrate:up
ALTER TABLE posts RENAME COLUMN body TO content;

-- migrate:down
ALTER TABLE posts RENAME COLUMN content TO body;
",
        ),
    ]);
    assert_eq!(
        help(&c, "SELECT body FROM posts"),
        "'body' was renamed to 'content' in db/migrations/20240312_rename_body.sql:2"
    );
}

#[test]
fn mysql_change_column_names_the_migration() {
    let c = catalog(
        &[
            ("V1__create.sql", "CREATE TABLE posts (id INT, body TEXT);"),
            (
                "V2__rename.sql",
                "ALTER TABLE posts\n  CHANGE COLUMN body content TEXT;",
            ),
        ],
        SqlDialect::MySQL,
    );
    let d = single(&c, SqlDialect::MySQL, "SELECT body FROM posts");
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was renamed to 'content' in V2__rename.sql:2")
    );
}

#[test]
fn changes_in_the_query_file_name_its_line() {
    let c = pg(&[CREATE]);
    let diags = Analyzer::new(&c).analyze(
        "SELECT body FROM posts;
ALTER TABLE posts RENAME COLUMN body TO content;
SELECT body FROM posts;
ALTER TABLE people
  RENAME TO persons;
SELECT name FROM people;
ALTER TABLE posts DROP COLUMN nick;
SELECT nick FROM posts;",
    );
    let helps: Vec<_> = diags.iter().map(|d| d.help.as_deref()).collect();
    assert_eq!(
        helps,
        vec![
            Some("'body' was renamed to 'content' at line 2 of this file"),
            Some("'people' was renamed to 'persons' at line 5 of this file"),
            Some("'nick' was dropped at line 7 of this file"),
        ],
        "{diags:#?}"
    );
}

#[test]
fn query_file_change_wins_over_schema_change() {
    let c = pg(&[
        CREATE,
        (
            "m/0002.sql",
            "ALTER TABLE posts RENAME COLUMN body TO text_body;",
        ),
    ]);
    let d = single(
        &c,
        SqlDialect::PostgreSQL,
        "ALTER TABLE posts RENAME COLUMN text_body TO content;\nSELECT body FROM posts;",
    );
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was renamed to 'content' at line 1 of this file")
    );
}

#[test]
fn schema_warnings_name_the_file_and_line() {
    let mut builder = SchemaBuilder::new();
    builder
        .parse_file(
            "CREATE TABLE posts (id INT);\n\nALTER TABLE missing ADD COLUMN x INT;",
            "db/migrations/0002.sql",
        )
        .unwrap();
    let (_, warnings) = builder.build();
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(
        warnings[0].message,
        "db/migrations/0002.sql:3: ALTER TABLE references table 'missing' which was not found in schema"
    );
    let span = warnings[0].span.expect("span");
    assert_eq!((span.line, span.column, span.offset), (3, 13, 42));
}
