// dbt / Jinja templated query files (`Templating::Jinja`)
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind};
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::Templating;

const SCHEMA: &str = r"
    CREATE TABLE users (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        created_at DATE
    );
    CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, total NUMERIC);
";

fn analyze_with(dialect: SqlDialect, templating: Templating, sql: &str) -> Vec<Diagnostic> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(SCHEMA).unwrap();
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .with_templating(templating)
        .analyze(sql)
}

fn analyze(sql: &str) -> Vec<Diagnostic> {
    analyze_with(SqlDialect::PostgreSQL, Templating::Jinja, sql)
}

fn assert_valid(sql: &str) {
    let diagnostics = analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Expected no diagnostics for:\n{sql}\ngot: {diagnostics:#?}"
    );
}

/// The only diagnostic: its kind, line and column
fn single(sql: &str) -> (DiagnosticKind, usize, usize) {
    let diagnostics = analyze(sql);
    assert_eq!(diagnostics.len(), 1, "{sql}\n{diagnostics:#?}");
    let span = diagnostics[0].span.unwrap();
    (diagnostics[0].kind, span.line, span.column)
}

#[test]
fn ref_and_source_are_tables_with_unknown_columns() {
    assert_valid("SELECT rental_id, amount FROM {{ ref('stg_rental') }}");
    assert_valid("SELECT r.rental_id FROM {{ ref('stg_rental') }} AS r WHERE r.x > 1");
    assert_valid(
        "SELECT u.name, s.anything\n\
         FROM users u\n\
         JOIN {{ source('app', 'events') }} s ON s.user_id = u.id",
    );
    assert_valid("SELECT * FROM {{ target.schema }}.users");
    assert_valid("SELECT a FROM {{ ref('a') }} JOIN {{ ref('b') }} USING (id)");
    assert_valid("SELECT id FROM {{ this }}");
}

#[test]
fn expressions_are_untyped_values() {
    assert_valid("SELECT id FROM users WHERE id > {{ var('min_id') }}");
    assert_valid("SELECT id FROM users WHERE created_at >= '{{ var(\"start\") }}'");
    assert_valid("SELECT {{ dbt_utils.star(ref('users')) }} FROM users");
    assert_valid("SELECT id, {{ cents_to_dollars('total') }} AS amount FROM orders");
}

#[test]
fn config_comments_and_blocks_are_masked() {
    assert_valid(
        "{{ config(materialized='incremental', unique_key='id') }}\n\
         {# a comment with {{ braces }} #}\n\
         {%- set cutoff = '2024-01-01' -%}\n\
         with src as (\n\
             select * from {{ ref('stg_orders') }}\n\
         )\n\
         select o.id, o.total\n\
         from orders o\n\
         {% if is_incremental() %}\n\
         where o.id > (select max(id) from {{ this }})\n\
         {% endif %}\n",
    );
    assert_valid(
        "select\n\
         {% for c in ['a', 'b'] %}\n\
           sum(total) as total_{{ c }},\n\
         {% endfor %}\n\
           count(*) as n\n\
         from orders",
    );
}

#[test]
fn real_tables_are_still_checked() {
    let sql = "{{ config(materialized='view') }}\n\
               SELECT u.nmae\n\
               FROM users u\n\
               JOIN {{ ref('stg_orders') }} o ON o.user_id = u.id";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 2, 10));
    let sql = "SELECT id FROM userz JOIN {{ ref('x') }} x ON x.id = userz.id";
    assert_eq!(single(sql), (DiagnosticKind::TableNotFound, 1, 16));
}

#[test]
fn types_of_real_columns_are_still_checked() {
    let sql = "SELECT id FROM users WHERE id = {{ var('x') }} AND name > 1";
    assert_eq!(single(sql).0, DiagnosticKind::TypeMismatch);
}

#[test]
fn locations_after_templates_are_original() {
    // A multi-byte comment and a multi-line tag before the typo
    let sql = "{# 日本語のコメント #}\n{{ config(\n  materialized='table'\n) }}\nSELECT {{ x }}, nmae FROM users";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 5, 17));
    let diagnostics = analyze(sql);
    let offset = diagnostics[0].span.unwrap().offset;
    assert!(sql[offset..].starts_with("nmae"), "{offset}");
}

#[test]
fn parse_errors_have_original_locations() {
    let sql = "{{ config(materialized='table') }}\nSELECT id,, name FROM {{ ref('a') }}";
    assert_eq!(single(sql), (DiagnosticKind::ParseError, 2, 11));
}

#[test]
fn templates_are_errors_without_templating() {
    let diagnostics = analyze_with(
        SqlDialect::PostgreSQL,
        Templating::None,
        "SELECT id FROM {{ ref('a') }}",
    );
    assert!(diagnostics
        .iter()
        .any(|d| d.kind == DiagnosticKind::ParseError));
}

#[test]
fn mysql_and_sqlite() {
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        let sql = "{{ config(materialized='table') }}\n\
                   SELECT o.id, x.y FROM orders o JOIN {{ ref('x') }} x ON x.id = o.user_id\n\
                   WHERE o.total > {{ var('min') }}";
        let diagnostics = analyze_with(dialect, Templating::Jinja, sql);
        assert!(diagnostics.is_empty(), "{dialect:?}: {diagnostics:#?}");
    }
}

#[test]
fn expressions_in_names_are_identifiers() {
    assert_valid(
        "select\n\
         {% for s in ['a', 'b'] %}\n\
           sum(case when o.id = {{ loop.index }} then 1 else 0 end) as {{ s }}_orders,\n\
           sum(total) as total_{{ s }},\n\
         {% endfor %}\n\
           o.{{ var('column') }}, {{ var('column') }}\n\
         from orders o",
    );
}
