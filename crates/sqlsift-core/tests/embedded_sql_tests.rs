// SQL from application code: sqlc query names
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::{Catalog, SchemaBuilder};

const SCHEMA: &str = include_str!("../../../tests/fixtures/embedded/schema.sql");

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::new();
    builder.parse(SCHEMA).unwrap();
    builder.build().0
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    Analyzer::new(&catalog()).analyze(sql)
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
