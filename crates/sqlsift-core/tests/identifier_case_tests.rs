//! Identifier case: PostgreSQL folds unquoted names to lowercase and compares quoted
//! names exactly, so a column created as `"authorId"` (Prisma style) is matched only
//! by `"authorId"`. Other dialects (and columns whose case doesn't matter) keep
//! matching names case-insensitively.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Templating;

/// A Prisma migration
const PRISMA_SCHEMA: &str = r#"
CREATE TABLE "User" ("id" TEXT NOT NULL, "email" TEXT NOT NULL, CONSTRAINT "User_pkey" PRIMARY KEY ("id"));
CREATE TABLE "Post" (
    "id" TEXT NOT NULL,
    "authorId" TEXT NOT NULL,
    "updatedAt" TIMESTAMP(3) NOT NULL,
    title TEXT,
    ViewCount INTEGER,
    CONSTRAINT "Post_pkey" PRIMARY KEY ("id")
);
"#;

fn analyze_with(dialect: SqlDialect, schema: &str, sql: &str) -> Vec<Diagnostic> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).expect("schema parses");
    let (catalog, _) = builder.build();
    Analyzer::with_dialect(&catalog, dialect)
        .with_templating(Templating::None)
        .analyze(sql)
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    analyze_with(SqlDialect::PostgreSQL, PRISMA_SCHEMA, sql)
}

#[track_caller]
fn assert_clean(sql: &str) {
    let diags = analyze(sql);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for {sql:?}, got: {diags:#?}"
    );
}

#[track_caller]
fn single(sql: &str) -> Diagnostic {
    let mut diags = analyze(sql);
    assert_eq!(
        diags.len(),
        1,
        "expected one diagnostic for {sql:?}, got: {diags:#?}"
    );
    diags.remove(0)
}

#[test]
fn quoted_references_with_the_same_case_match() {
    assert_clean(r#"SELECT "id", "authorId", p."updatedAt" FROM "Post" p WHERE "authorId" = $1"#);
    assert_clean(
        r#"SELECT "Post"."authorId" FROM "Post" JOIN "User" ON "User"."id" = "Post"."authorId""#,
    );
    assert_clean(r#"INSERT INTO "Post" ("id", "authorId", "updatedAt") VALUES ($1, $2, now())"#);
    assert_clean(r#"UPDATE "Post" SET "updatedAt" = now() WHERE "id" = $1"#);
}

#[test]
fn lowercase_columns_match_any_case() {
    // `"id"` is all lowercase: `id`, `ID` and `Id` all fold to it
    assert_clean(r#"SELECT id, ID, Id, p.ID FROM "Post" p"#);
    // Unquoted in the DDL: folded to lowercase by PostgreSQL, so any unquoted case
    // matches
    assert_clean(r#"SELECT title, TITLE, viewcount, ViewCount, VIEWCOUNT FROM "Post""#);
}

#[test]
fn unquoted_reference_to_mixed_case_column_is_reported() {
    let diag = single(r#"SELECT authorId FROM "Post""#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
    assert!(diag.message.contains("'authorid'"), "{}", diag.message);
    assert_eq!(
        diag.help.as_deref(),
        Some(r#"Column names are case-sensitive when quoted; did you mean "authorId"?"#)
    );

    // Qualified, all lowercase, and in WHERE
    let diag = single(r#"SELECT p.authorid FROM "Post" p"#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
    let diag = single(r#"SELECT "id" FROM "Post" WHERE updatedAt > now()"#);
    assert!(diag.help.unwrap().contains(r#""updatedAt""#));
}

#[test]
fn quoted_reference_with_another_case_is_reported() {
    let diag = single(r#"SELECT "AuthorId" FROM "Post""#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
    assert!(diag.help.unwrap().contains(r#""authorId""#));
}

#[test]
fn insert_and_update_targets_follow_the_same_rule() {
    let diag = single(r#"INSERT INTO "Post" ("id", authorId, "updatedAt") VALUES ($1, $2, now())"#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
    let diag = single(r#"UPDATE "Post" SET updatedAt = now() WHERE "id" = $1"#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn output_aliases_and_derived_columns_are_not_checked() {
    // ORDER BY an output alias
    assert_clean(r#"SELECT "authorId" AS authorId FROM "Post" ORDER BY authorId"#);
    // Columns of CTEs and subqueries keep matching case-insensitively
    assert_clean(r#"WITH c AS (SELECT "authorId" FROM "Post") SELECT authorId FROM c"#);
    assert_clean(r#"SELECT s.authorId FROM (SELECT "authorId" FROM "Post") s"#);
}

#[test]
fn a_missing_column_used_twice_is_reported_once() {
    // ORDER BY "missing" refers to the output column of the same name, which is the
    // erroneous select item itself
    let diag = single(r#"SELECT "missing" FROM "Post" ORDER BY "missing""#);
    assert_eq!(diag.kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn table_name_case_hint() {
    let diag = single(r#"SELECT "id" FROM post"#);
    assert_eq!(diag.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        diag.help.as_deref(),
        Some(r#"Table names are case-sensitive when quoted; did you mean "Post"?"#)
    );
}

#[test]
fn alter_table_added_and_renamed_columns() {
    let schema = r#"
        CREATE TABLE "Post" ("id" TEXT, title TEXT);
        ALTER TABLE "Post" ADD COLUMN "publishedAt" TIMESTAMP;
        ALTER TABLE "Post" RENAME COLUMN title TO "Title";
    "#;
    let diags = analyze_with(
        SqlDialect::PostgreSQL,
        schema,
        r#"SELECT "publishedAt", "Title" FROM "Post""#,
    );
    assert!(diags.is_empty(), "{diags:#?}");
    let diags = analyze_with(
        SqlDialect::PostgreSQL,
        schema,
        r#"SELECT publishedAt, title FROM "Post""#,
    );
    assert_eq!(diags.len(), 2, "{diags:#?}");
    assert!(diags
        .iter()
        .all(|d| d.kind == DiagnosticKind::ColumnNotFound));
}

#[test]
fn other_dialects_match_case_insensitively() {
    // Redshift folds quoted names too (`enable_case_sensitive_identifier` is off by
    // default); SQLite and Snowflake are left as they are
    for dialect in [
        SqlDialect::Redshift,
        SqlDialect::SQLite,
        SqlDialect::Snowflake,
    ] {
        let diags = analyze_with(
            dialect,
            r#"CREATE TABLE "Post" ("id" TEXT, "authorId" TEXT);"#,
            r#"SELECT authorId, AUTHORID FROM "Post""#,
        );
        assert!(diags.is_empty(), "{dialect:?}: {diags:#?}");
    }
    let diags = analyze_with(
        SqlDialect::MySQL,
        "CREATE TABLE `Post` (`id` TEXT, `authorId` TEXT);",
        "SELECT authorId, AUTHORID FROM `Post`",
    );
    assert!(diags.is_empty(), "{diags:#?}");
}
