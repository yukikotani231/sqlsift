// SQL from application code: sqlc query names and TypeScript tagged templates
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const SCHEMA: &str = include_str!("../../../tests/fixtures/embedded/schema.sql");
const QUERIES_TS: &str = include_str!("../../../tests/fixtures/embedded/queries.ts");

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).unwrap();
    builder.build().0
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    Analyzer::new(&catalog()).analyze(sql)
}

fn analyze_ts(source: &str, tags: &[&str]) -> Vec<Diagnostic> {
    Analyzer::new(&catalog()).analyze_embedded(source, tags)
}

/// (kind, line, column) of each diagnostic
fn locations(diagnostics: &[Diagnostic]) -> Vec<(DiagnosticKind, usize, usize)> {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("span");
            (d.kind, span.line, span.column)
        })
        .collect()
}

#[test]
fn tagged_templates_are_checked_at_their_location() {
    assert_eq!(
        locations(&analyze_ts(QUERIES_TS, &["sql"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 16, 28),
            (DiagnosticKind::TableNotFound, 31, 10),
        ]
    );
}

#[test]
fn configured_tags_select_the_templates() {
    assert_eq!(
        locations(&analyze_ts(QUERIES_TS, &["sql", "$queryRaw"])),
        vec![
            (DiagnosticKind::ColumnNotFound, 16, 28),
            (DiagnosticKind::TableNotFound, 31, 10),
            (DiagnosticKind::ColumnNotFound, 35, 83),
        ]
    );
    assert!(analyze_ts(QUERIES_TS, &["query"]).is_empty());
}

#[test]
fn byte_offsets_point_into_the_original_source() {
    let source = "const s = 'Ünïcode';\nconst q = sql`SELECT nme FROM authors`;\n";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert_eq!(diagnostics.len(), 1);
    let span = diagnostics[0].span.unwrap();
    assert_eq!((span.line, span.column), (2, 22));
    assert_eq!(&source[span.offset..span.offset + 3], "nme");
}

#[test]
fn interpolations_are_untyped_values() {
    let source = r"
        sql`SELECT id FROM posts WHERE id = ${id} AND title = ${t} LIMIT ${n}`;
        sql`INSERT INTO posts (author_id, title, body) VALUES (${a}, ${b}, ${c})`;
        sql`UPDATE posts SET published = ${p} WHERE id IN ${ids}`;
    ";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn interpolated_table_names_are_not_reported() {
    let source = "sql`SELECT anything FROM ${table} WHERE x = 1`;\n\
                  sql`INSERT INTO ${t} (a) VALUES (1)`;\n\
                  sql`SELECT anything FROM ${schema}.posts`;\n\
                  sql`SELECT a.nme FROM authors a JOIN ${t} ON true`;";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 4, 14)]
    );
}

#[test]
fn templates_are_separate_statements() {
    // A `--` comment at the end of a template doesn't hide the next one
    let source = "sql`SELECT id FROM posts -- all posts`;\nsql`SELECT nme FROM authors`;";
    assert_eq!(
        locations(&analyze_ts(source, &["sql"])),
        vec![(DiagnosticKind::ColumnNotFound, 2, 12)]
    );
}

#[test]
fn parse_errors_point_into_the_template() {
    let source = "const a = 1;\nconst q = sql`SELEC id FROM posts`;";
    let diagnostics = analyze_ts(source, &["sql"]);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ParseError);
    assert_eq!(diagnostics[0].span.unwrap().line, 2);
}

#[test]
fn files_without_tagged_templates_have_no_diagnostics() {
    let source = "export const x = `SELECT nme FROM nowhere`;\n// sql`SELECT 1`\n";
    assert!(analyze_ts(source, &["sql"]).is_empty());
}

#[test]
fn mysql_placeholders() {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder
        .parse("CREATE TABLE posts (id INT PRIMARY KEY, title TEXT);")
        .unwrap();
    let catalog = builder.build().0;
    let source = "sql`SELECT title FROM posts WHERE id = ${id} AND id IN ${ids}`;\n\
                  sql`SELECT titel FROM posts`;";
    let diagnostics =
        Analyzer::with_dialect(&catalog, SqlDialect::MySQL).analyze_embedded(source, &["sql"]);
    assert_eq!(
        locations(&diagnostics),
        vec![(DiagnosticKind::ColumnNotFound, 2, 12)]
    );
}

#[test]
fn inline_directives_work_inside_templates() {
    let source = "sql`\n  -- sqlsift:disable E0002\n  SELECT nme FROM authors\n`;";
    assert!(analyze_ts(source, &["sql"]).is_empty());
}

/// (kind, line, column, query name) of each diagnostic
fn summary(diagnostics: &[Diagnostic]) -> Vec<(DiagnosticKind, usize, usize, Option<&str>)> {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("span");
            (d.kind, span.line, span.column, d.query_name.as_deref())
        })
        .collect()
}

#[test]
fn sqlc_query_names_are_attached_to_diagnostics() {
    let diagnostics = analyze(include_str!("../../../tests/fixtures/embedded/queries.sql"));
    assert_eq!(
        summary(&diagnostics),
        vec![
            (DiagnosticKind::ColumnNotFound, 8, 12, Some("ListPosts")),
            (DiagnosticKind::TableNotFound, 18, 22, Some("GetAuthor")),
        ]
    );
}

#[test]
fn statements_before_the_first_name_have_no_query_name() {
    let diagnostics =
        analyze("SELECT nme FROM authors;\n-- name: A :one\nSELECT nme FROM authors;");
    assert_eq!(
        summary(&diagnostics),
        vec![
            (DiagnosticKind::ColumnNotFound, 1, 8, None),
            (DiagnosticKind::ColumnNotFound, 3, 8, Some("A")),
        ]
    );
}

#[test]
fn parse_errors_get_the_query_name() {
    let diagnostics = analyze("-- name: Broken :exec\nSELEC 1;\n\n-- name: Fine :one\nSELECT 1;");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ParseError);
    assert_eq!(diagnostics[0].query_name.as_deref(), Some("Broken"));
}
