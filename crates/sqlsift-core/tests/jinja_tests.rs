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
    CREATE SCHEMA raw;
    CREATE TABLE raw.customers (id INTEGER PRIMARY KEY, email TEXT);
    CREATE TABLE raw.payments (id INTEGER PRIMARY KEY, order_id INTEGER, amount NUMERIC);
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

// Issue #120

#[test]
fn loop_separators_are_dropped() {
    assert_valid(
        "select order_id,\n\
         {% for m in ['a', 'b'] %}\n\
           sum(amount) as {{ m }}_amount{% if not loop.last %},{% endif %}\n\
         {% endfor %}\n\
         from raw.payments group by 1",
    );
    assert_valid(
        "select\n\
         {%- for c in ['id', 'amount'] %}\n\
           {{ c }}{{ ',' if not loop.last }}\n\
         {%- endfor %}\n\
         from raw.payments",
    );
    assert_valid(
        "{% for t in ['a', 'b'] %}\n\
         select id from users\n\
         {% if not loop.last %} union all {% endif %}\n\
         {% endfor %}",
    );
}

#[test]
fn only_the_first_branch_is_checked() {
    assert_valid(
        "select id, {% if target.name == 'prod' %} name {% else %} 'redacted' as name {% endif %}\n\
         from users",
    );
    assert_valid(
        "select id from users\n\
         {% if var('a') %} where id > 1 {% elif var('b') %} where id > 2 {% else %} {% endif %}",
    );
    // The kept branch is still checked
    let sql = "select {% if x %} nmae {% else %} name {% endif %} from users";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 1, 19));
}

#[test]
fn set_call_and_macro_bodies_are_skipped() {
    assert_valid(
        "{%- set status_list -%} 'placed', 'shipped' {%- endset -%}\n\
         select id from users where name in ({{ status_list }})",
    );
    assert_valid(
        "{%- call statement('max_date', fetch_result=True) -%}\n\
           select max(created_at) from users\n\
         {%- endcall -%}\n\
         select id from users where created_at = '{{ max_date }}'",
    );
    assert_valid(
        "{% macro cents_to_dollars(column_name, scale=2) %}\n\
           ({{ column_name }} / 100)::numeric(16, {{ scale }})\n\
         {% endmacro %}\n",
    );
    assert_valid("select {% raw %}'{{ not jinja }}'{% endraw %} as a from users");
}

#[test]
fn template_casts_are_unknown_types() {
    assert_valid("select id::{{ dbt.type_bigint() }} from orders");
    assert_valid("select cast(id as {{ dbt.type_string() }}) from orders");
}

#[test]
fn quoted_expressions_are_untyped() {
    assert_valid("select id from orders where id = '{{ var(\"x\") }}'");
    assert_valid("select id from orders where id = 'prefix_{{ var(\"x\") }}'");
    assert_valid(
        "select id from users where created_at > now() - interval '{{ var(\"n\") }} days'",
    );
    // A plain string is still typed
    let sql = "select id from orders where id = 'x' and total > '{{ var(\"t\") }}'";
    assert_eq!(single(sql).0, DiagnosticKind::TypeMismatch);
}

#[test]
fn parse_errors_show_the_template_text() {
    let diagnostics =
        analyze("with spine as (\n  {{ dbt_utils.date_spine(datepart='day') }}\n)\nselect 1");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    let d = &diagnostics[0];
    assert_eq!(d.kind, DiagnosticKind::ParseError);
    assert!(!d.message.contains("$1"), "{}", d.message);
    assert!(
        d.message
            .contains("found: Jinja expression {{ dbt_utils.date_spine(datepart='day') }}"),
        "{}",
        d.message
    );
    assert!(d.help.as_deref().unwrap().contains("sqlsift:disable-file"));
}

#[test]
fn template_without_templating_suggests_it() {
    let diagnostics = analyze_with(
        SqlDialect::PostgreSQL,
        Templating::None,
        "select id from {{ ref('a') }}",
    );
    let help = diagnostics[0].help.as_deref().unwrap_or_default();
    assert!(help.contains("--templating jinja"), "{diagnostics:#?}");
    // Plain parse errors get no such hint
    let diagnostics = analyze_with(
        SqlDialect::PostgreSQL,
        Templating::None,
        "select id,, from users",
    );
    assert!(diagnostics[0].help.is_none(), "{diagnostics:#?}");
}

#[test]
fn jinja_comment_directives_work() {
    assert_valid("{# sqlsift:disable-file #}\nselect bogus from users");
    assert_valid("{#- sqlsift:disable-file E0002 -#}\nselect bogus from users");
    assert_valid("{# sqlsift:disable E0002 #}\nselect bogus from users");
    assert_valid("select bogus from users {# sqlsift:disable column-not-found #}");
    // A directive applies to the next SQL line, past lines of template tags
    assert_valid("{# sqlsift:disable E0002 #}\n{% if x %}\nselect bogus from users\n{% endif %}");
    let sql = "{# sqlsift:disable E0001 #}\nselect bogus from users";
    assert_eq!(single(sql).0, DiagnosticKind::ColumnNotFound);
}

#[test]
fn sources_in_the_schema_are_checked() {
    let sql = "select bogus_col from {{ source('raw', 'customers') }}";
    assert_eq!(single(sql), (DiagnosticKind::ColumnNotFound, 1, 8));
    assert_valid("select c.id, c.email from {{ source(\"raw\", \"customers\") }} as c");
    // A source the schema doesn't have is a table with unknown columns
    assert_valid("select anything from {{ source('raw', 'unknown') }}");
}
