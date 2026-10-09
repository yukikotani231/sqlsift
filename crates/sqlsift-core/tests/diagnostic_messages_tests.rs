//! Message and help text of diagnostics that explain *why* a name doesn't resolve:
//! schema-qualified table suggestions, the relation an unqualified column was looked
//! up in, renamed and dropped columns, inline enum columns, skipped schema statements
//! and misspelled rule names in inline directives.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, FormerColumn, QualifiedName, SchemaBuilder};

fn catalog(schema: &str, dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).expect("schema parses");
    builder.build().0
}

fn pg(schema: &str) -> Catalog {
    catalog(schema, SqlDialect::PostgreSQL)
}

/// Analyze and expect exactly one diagnostic
#[track_caller]
fn single(catalog: &Catalog, dialect: SqlDialect, sql: &str) -> Diagnostic {
    let mut diags = Analyzer::with_dialect(catalog, dialect).analyze(sql);
    assert_eq!(diags.len(), 1, "for {sql:?}: {diags:#?}");
    diags.remove(0)
}

#[track_caller]
fn single_pg(catalog: &Catalog, sql: &str) -> Diagnostic {
    single(catalog, SqlDialect::PostgreSQL, sql)
}

// ---------------------------------------------------------------------
// #84: table suggestions in other schemas are schema-qualified
// ---------------------------------------------------------------------

const SCHEMAS: &str = "
    CREATE SCHEMA billing;
    CREATE TABLE billing.charges (id INT PRIMARY KEY, amount INT);
    CREATE TABLE public.users (id INT PRIMARY KEY, name TEXT);
";

#[test]
fn table_in_another_schema_is_suggested_qualified() {
    let c = pg(SCHEMAS);
    let d = single_pg(&c, "SELECT amount FROM charges");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.message, "Table 'charges' not found");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'billing.charges'?"));

    let d = single_pg(&c, "SELECT amount FROM charge");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'billing.charges'?"));
}

#[test]
fn table_in_default_schema_is_suggested_unqualified() {
    let c = pg(SCHEMAS);
    let d = single_pg(&c, "SELECT id FROM billing.users");
    assert_eq!(d.message, "Table 'billing.users' not found");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'users'?"));

    let d = single_pg(&c, "SELECT id FROM usrs");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'users'?"));
}

#[test]
fn table_in_the_searched_schema_wins_over_other_schemas() {
    let c = pg("
        CREATE SCHEMA billing;
        CREATE TABLE billing.charges (id INT);
        CREATE TABLE charges (id INT);
        ");
    let d = single_pg(&c, "SELECT id FROM billing.charge");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'billing.charges'?"));
    let d = single_pg(&c, "SELECT id FROM charge");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'charges'?"));
}

// ---------------------------------------------------------------------
// #88: unqualified columns name the relation; renamed and dropped columns
// ---------------------------------------------------------------------

const POSTS: &str = "
    CREATE TABLE posts (id INT PRIMARY KEY, body TEXT, legacy_flag BOOLEAN);
    CREATE TABLE authors (id INT PRIMARY KEY, name TEXT);
";

#[test]
fn unqualified_column_names_the_only_relation() {
    let c = pg(POSTS);
    let d = single_pg(&c, "SELECT title FROM posts");
    assert_eq!(d.message, "Column 'title' not found in table 'posts'");

    // The alias is not the table name: the table is named
    let d = single_pg(&c, "SELECT title FROM posts p");
    assert_eq!(d.message, "Column 'title' not found in table 'posts'");

    let d = single_pg(
        &c,
        "WITH recent AS (SELECT id FROM posts) SELECT title FROM recent",
    );
    assert_eq!(d.message, "Column 'title' not found in CTE 'recent'");

    let d = single_pg(&c, "SELECT title FROM (SELECT id FROM posts) AS s");
    assert_eq!(d.message, "Column 'title' not found in subquery 's'");
}

#[test]
fn unqualified_column_with_several_relations_names_none() {
    let c = pg(POSTS);
    let d = single_pg(&c, "SELECT title FROM posts, authors");
    assert_eq!(d.message, "Column 'title' not found");

    // A correlated subquery also sees the outer query's relations
    let d = single_pg(
        &c,
        "SELECT id FROM authors a WHERE EXISTS (SELECT 1 FROM posts WHERE title = a.name)",
    );
    assert_eq!(d.message, "Column 'title' not found");
}

#[test]
fn renamed_column_says_what_it_was_renamed_to() {
    let c = pg(&format!(
        "{POSTS} ALTER TABLE posts RENAME COLUMN body TO content;"
    ));
    let help = Some("'body' was renamed to 'content' by ALTER TABLE in the schema");

    let d = single_pg(&c, "SELECT body FROM posts");
    assert_eq!(d.message, "Column 'body' not found in table 'posts'");
    assert_eq!(d.help.as_deref(), help);

    let d = single_pg(&c, "SELECT p.body FROM posts p");
    assert_eq!(d.message, "Column 'body' not found in table 'posts'");
    assert_eq!(d.help.as_deref(), help);

    let d = single_pg(
        &c,
        "SELECT a.name FROM authors a JOIN posts p ON p.id = a.id WHERE body = ''",
    );
    assert_eq!(d.help.as_deref(), help);

    let d = single_pg(&c, "INSERT INTO posts (id, body) VALUES (1, 'x')");
    assert_eq!(d.help.as_deref(), help);

    let d = single_pg(&c, "UPDATE posts SET body = 'x'");
    assert_eq!(d.help.as_deref(), help);
}

#[test]
fn renames_are_followed_to_the_current_name() {
    let c = pg(&format!(
        "{POSTS}
        ALTER TABLE posts RENAME COLUMN body TO text_body;
        ALTER TABLE posts RENAME COLUMN text_body TO content;"
    ));
    let d = single_pg(&c, "SELECT body FROM posts");
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was renamed to 'content' by ALTER TABLE in the schema")
    );
    let d = single_pg(&c, "SELECT text_body FROM posts");
    assert_eq!(
        d.help.as_deref(),
        Some("'text_body' was renamed to 'content' by ALTER TABLE in the schema")
    );
}

#[test]
fn dropped_column_says_it_was_dropped() {
    let c = pg(&format!(
        "{POSTS} ALTER TABLE posts DROP COLUMN legacy_flag;"
    ));
    let d = single_pg(&c, "SELECT legacy_flag FROM posts");
    assert_eq!(d.message, "Column 'legacy_flag' not found in table 'posts'");
    assert_eq!(
        d.help.as_deref(),
        Some("'legacy_flag' was dropped by ALTER TABLE in the schema")
    );

    // Renamed, then dropped under the new name
    let c = pg(&format!(
        "{POSTS}
        ALTER TABLE posts RENAME COLUMN body TO content;
        ALTER TABLE posts DROP COLUMN content;"
    ));
    let d = single_pg(&c, "SELECT body FROM posts");
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was dropped by ALTER TABLE in the schema")
    );
}

#[test]
fn readded_column_is_no_longer_a_former_column() {
    let c = pg(&format!(
        "{POSTS}
        ALTER TABLE posts DROP COLUMN body;
        ALTER TABLE posts ADD COLUMN body TEXT;"
    ));
    let posts = c.get_table(&QualifiedName::new("posts")).unwrap();
    assert!(
        posts.former_columns.is_empty(),
        "{:?}",
        posts.former_columns
    );
    assert!(Analyzer::new(&c)
        .analyze("SELECT body FROM posts")
        .is_empty());
}

#[test]
fn mysql_change_column_records_the_rename() {
    let c = catalog(
        "CREATE TABLE posts (id INT, body TEXT);
         ALTER TABLE posts CHANGE COLUMN body content TEXT;",
        SqlDialect::MySQL,
    );
    let posts = c.get_table(&QualifiedName::new("posts")).unwrap();
    assert_eq!(
        posts.former_columns,
        vec![FormerColumn {
            name: "body".to_string(),
            renamed_to: Some("content".to_string()),
        }]
    );
    let d = single(&c, SqlDialect::MySQL, "SELECT body FROM posts");
    assert_eq!(
        d.help.as_deref(),
        Some("'body' was renamed to 'content' by ALTER TABLE in the schema")
    );
}

// ---------------------------------------------------------------------
// #89: inline ENUM(...) values name the column
// ---------------------------------------------------------------------

const CUSTOMERS: &str = "
    CREATE TABLE customers (
        id INT AUTO_INCREMENT PRIMARY KEY,
        plan ENUM('free', 'pro', 'enterprise') NOT NULL
    );
";

#[test]
fn inline_enum_value_names_the_column() {
    let c = catalog(CUSTOMERS, SqlDialect::MySQL);
    let expected = "Invalid value 'premium' for enum column 'customers.plan'";
    for sql in [
        "SELECT id FROM customers WHERE plan = 'premium'",
        "SELECT id FROM customers c WHERE c.plan = 'premium'",
        "SELECT id FROM customers WHERE 'premium' = plan",
        "INSERT INTO customers (plan) VALUES ('premium')",
        "UPDATE customers SET plan = 'premium'",
    ] {
        let d = single(&c, SqlDialect::MySQL, sql);
        assert_eq!(d.kind, DiagnosticKind::TypeMismatch, "{sql}");
        assert_eq!(d.message, expected, "{sql}");
        assert_eq!(
            d.help.as_deref(),
            Some("Valid values: 'free', 'pro', 'enterprise'"),
            "{sql}"
        );
    }
}

#[test]
fn named_enum_keeps_the_type_name() {
    let c = pg("
        CREATE TYPE post_status AS ENUM ('draft', 'published');
        CREATE TABLE posts (id INT, status post_status);
        ");
    let d = single_pg(&c, "SELECT id FROM posts WHERE status = 'archived'");
    assert_eq!(
        d.message,
        "Invalid value 'archived' for enum type 'post_status'"
    );
}

// ---------------------------------------------------------------------
// #99: E0001 help when there is no similar table
// ---------------------------------------------------------------------

#[test]
fn missing_table_points_at_sqlsift_schema() {
    let c = pg(POSTS);
    let d = single_pg(&c, "SELECT 1 FROM zzzzzz");
    assert_eq!(
        d.help.as_deref(),
        Some(
            "Check that the table exists in your schema definition; \
             run `sqlsift schema <schema files>` to list the tables that were loaded"
        )
    );
}

#[test]
fn missing_table_in_unknown_schema_says_so() {
    let c = pg(POSTS);
    let d = single_pg(&c, "SELECT 1 FROM analytics.events");
    assert_eq!(
        d.help.as_deref(),
        Some(
            "Schema 'analytics' has no tables in the schema input; \
             check that the schema files that define it are included"
        )
    );
}

/// A CREATE TABLE sqlparser can't parse (skipped with a warning)
const UNPARSEABLE: &str = "
    CREATE TABLE posts (id INT PRIMARY KEY);
    CREATE TABLE events (id INT, payload TEXT) PARTITION BY RANGE (id) USING something weird;
";

#[test]
fn missing_table_whose_definition_was_skipped_says_so() {
    let mut builder = SchemaBuilder::new();
    builder.parse(UNPARSEABLE).expect("schema parses");
    let (c, warnings) = builder.build();
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(c.skipped_definitions.len(), 1);

    let d = single_pg(&c, "SELECT payload FROM events");
    assert_eq!(
        d.help.as_deref(),
        Some(
            "A CREATE TABLE statement for 'events' in the schema could not be parsed \
             and was skipped (see the schema warnings)"
        )
    );

    let d = single_pg(&c, "SELECT 1 FROM zzzzzz");
    assert_eq!(
        d.help.as_deref(),
        Some(
            "1 schema statement(s) could not be parsed and were skipped (see the schema \
             warnings), so the table may be defined in one of them; run `sqlsift schema \
             <schema files>` to list the tables that were loaded"
        )
    );
}

// ---------------------------------------------------------------------
// #90: misspelled rule names in inline directives
// ---------------------------------------------------------------------

#[test]
fn misspelled_rule_in_disable_comment_is_explained() {
    let c = pg(POSTS);
    let d = single_pg(
        &c,
        "SELECT title FROM posts -- sqlsift:disable colum-not-found",
    );
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(
        d.help.as_deref(),
        Some(
            "'colum-not-found' in the sqlsift:disable comment is not a rule. \
             Did you mean 'column-not-found'?"
        )
    );

    // Appended to the diagnostic's own help
    let d = single_pg(
        &c,
        "-- sqlsift:disable ambigous-column\nSELECT bdy FROM posts",
    );
    assert_eq!(
        d.help.as_deref(),
        Some(
            "Did you mean 'body'?\n\
             'ambigous-column' in the sqlsift:disable comment is not a rule. \
             Did you mean 'ambiguous-column'?"
        )
    );

    // Correctly spelled names don't add anything
    let d = single_pg(&c, "SELECT bdy FROM posts -- sqlsift:disable E0006");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'body'?"));
}

#[test]
fn swapped_adjacent_characters_are_suggested() {
    let c = pg("CREATE TABLE events (id int, kind text, day date);");
    let d = single_pg(&c, "SELECT kidn FROM events");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(d.help.as_deref(), Some("Did you mean 'kind'?"));

    let d = single_pg(&c, "SELECT dya FROM events");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(d.help.as_deref(), Some("Did you mean 'day'?"));
}
