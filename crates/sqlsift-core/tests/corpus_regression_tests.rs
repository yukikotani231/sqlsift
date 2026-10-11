//! Minimal reproductions distilled from the real-world corpus (`tests/corpus/`):
//! dbt packages (jaffle_shop, fivetran dbt_* packages, dbt-utils), mozilla/bigquery-etl,
//! Snowflake-Labs/sf-samples and awslabs/amazon-redshift-utils.
//!
//! Every query here runs in production, so sqlsift must not report anything on it.
//! The `#[ignore]`d tests are false positives (or parse gaps) the corpus found that are
//! not fixed yet; each names its class in the corpus report. Un-ignore a test when its
//! class is fixed. The last tests check that the breakages the mutation testing injects
//! are found.

use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::{Analyzer, Diagnostic, DiagnosticKind, SqlDialect, Templating};

fn check_with(
    dialect: SqlDialect,
    templating: Templating,
    schema: &str,
    sql: &str,
) -> Vec<Diagnostic> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(schema).unwrap();
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .with_templating(templating)
        .analyze(sql)
}

fn check(dialect: SqlDialect, schema: &str, sql: &str) -> Vec<Diagnostic> {
    check_with(dialect, Templating::None, schema, sql)
}

fn dbt(dialect: SqlDialect, schema: &str, sql: &str) -> Vec<Diagnostic> {
    check_with(dialect, Templating::Jinja, schema, sql)
}

#[track_caller]
fn assert_clean(diagnostics: &[Diagnostic]) {
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {:#?}",
        diagnostics
            .iter()
            .map(|d| format!("{}: {}", d.kind.code(), d.message))
            .collect::<Vec<_>>()
    );
}

fn kinds(diagnostics: &[Diagnostic]) -> Vec<DiagnosticKind> {
    diagnostics.iter().map(|d| d.kind).collect()
}

// --- clean in the corpus (keep them clean) ------------------------------------------

/// jaffle_shop `stg_payments` on Snowflake: arithmetic on NUMBER columns (catalog.json types)
#[test]
fn snowflake_number_arithmetic_is_clean() {
    assert_clean(&check(
        SqlDialect::Snowflake,
        "CREATE TABLE raw_payments (id NUMBER(38,0), amount NUMBER(38,0), payment_method TEXT);",
        "SELECT id AS payment_id, amount / 100 AS amount FROM raw_payments;",
    ));
}

/// sf-samples workload queries: VARIANT paths, FLATTEN, QUALIFY, DATEADD/DATEDIFF parts
#[test]
fn snowflake_semi_structured_and_date_parts_are_clean() {
    let schema = "CREATE TABLE t (id INT, payload VARIANT, ts TIMESTAMP_NTZ, k INT);";
    assert_clean(&check(
        SqlDialect::Snowflake,
        schema,
        "SELECT t.id, f.value:name::string AS name, f.index \
         FROM t, LATERAL FLATTEN(input => t.payload:items) f;",
    ));
    assert_clean(&check(
        SqlDialect::Snowflake,
        schema,
        "SELECT id, IFF(k > 1, 'a', 'b') AS x, DATEADD(day, -30, DATE_TRUNC(day, ts)) AS d, \
         DATEDIFF(hour, ts, ts) AS h FROM t \
         QUALIFY ROW_NUMBER() OVER (PARTITION BY id ORDER BY ts DESC) = 1;",
    ));
}

/// bigquery-etl: STRUCT field paths, date parts as identifiers, `* EXCEPT`, BY NAME, UNNEST
#[test]
fn bigquery_struct_paths_date_parts_and_set_operations_are_clean() {
    let schema = "CREATE TABLE ds.t (a INT64, d DATE, info STRUCT<x INT64, y STRING>, \
                  events ARRAY<STRUCT<name STRING, ts TIMESTAMP>>);\n\
                  CREATE TABLE ds.u (d DATE, a INT64, c STRING);";
    assert_clean(&check(
        SqlDialect::BigQuery,
        schema,
        "SELECT a, info.x, t.info.y, DATE_TRUNC(d, ISOWEEK) AS w, DATE_DIFF(d, d, DAY) AS n \
         FROM ds.t;",
    ));
    assert_clean(&check(
        SqlDialect::BigQuery,
        schema,
        "SELECT * EXCEPT (info, events) FROM ds.t UNION ALL BY NAME SELECT a, d FROM ds.u;",
    ));
    assert_clean(&check(
        SqlDialect::BigQuery,
        schema,
        "SELECT a, e.name FROM ds.t CROSS JOIN UNNEST(events) AS e;",
    ));
}

/// amazon-redshift-utils: Redshift functions
#[test]
fn redshift_functions_are_clean() {
    assert_clean(&check(
        SqlDialect::Redshift,
        "CREATE TABLE t (dt TIMESTAMP, a INT, s VARCHAR(10));",
        "SELECT CONVERT_TIMEZONE('UTC', 'US/Pacific', dt), NVL(a, 0), 'x' + s, \
         DATEADD(day, 1, dt), DATEDIFF(day, dt, dt) FROM t;",
    ));
}

/// fivetran staging models: `{{ config() }}` first, CTE chain over `{{ ref() }}`,
/// `{{ dbt.type_string() }}` casts, `{% if var() %}` branches
#[test]
fn fivetran_style_staging_model_is_clean() {
    let schema = "CREATE TABLE raw.charge (id TEXT, amount BIGINT, created TIMESTAMP);";
    let model = r"{{ config(enabled=var('stripe__using_charges', True)) }}

with base as (

    select *
    from {{ ref('stg_stripe__charge_tmp') }}
),

fields as (

    select
        {{ fivetran_utils.fill_staging_columns(
                source_columns=adapter.get_columns_in_relation(ref('stg_stripe__charge_tmp')),
                staging_columns=get_charge_columns()
            )
        }}
    from base
),

final as (

    select
        cast(id as {{ dbt.type_string() }}) as charge_id,
        amount,
        created as created_at
    from fields
    {% if var('stripe__using_livemode', True) %}
    where amount > 0
    {% endif %}
)

select *
from final
";
    for dialect in [
        SqlDialect::PostgreSQL,
        SqlDialect::Snowflake,
        SqlDialect::BigQuery,
        SqlDialect::Redshift,
        SqlDialect::Databricks,
    ] {
        assert_clean(&dbt(dialect, schema, model));
    }
}

// --- known false positives and parse gaps (not fixed yet) ---------------------------

/// Class dbt-type-macro-arith: `cast(x as {{ dbt.type_numeric() }})` is a placeholder
/// type, reported as non-numeric in arithmetic (fivetran stripe, hubspot, shopify,
/// fivetran_log, jira, zendesk)
#[test]
#[ignore = "known false positive (corpus report: dbt-type-macro-arith)"]
fn dbt_type_macro_in_arithmetic() {
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        "CREATE TABLE t (a INT, b INT);",
        "select cast(a as {{ dbt.type_numeric() }}) + cast(b as {{ dbt.type_numeric() }}) as total from t",
    ));
}

/// Class dbt-config-after-comment: `{{ config() }}` after a `--` comment line is not
/// treated as the start of the statement (150 fivetran models fail to parse)
#[test]
#[ignore = "known parse gap (corpus report: dbt-config-after-comment)"]
fn dbt_config_after_comment() {
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        "CREATE TABLE t (a INT);",
        "-- To disable this model, set the var to False.\n\
         {{ config(enabled=var('x', True)) }}\n\nselect a from t",
    ));
}

/// Class dbt-clause-macro: macros that expand to `, column` lists or whole clauses
/// (`apply_source_relation`, `partition_by_source_relation`, `dbt_utils.group_by`)
#[test]
#[ignore = "known parse gap (corpus report: dbt-clause-macro)"]
fn dbt_macros_expanding_to_columns_or_clauses() {
    let schema = "CREATE TABLE t (a INT, b INT);";
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        schema,
        "select\n  a\n  {{ fivetran_utils.apply_source_relation() }}\nfrom t",
    ));
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        schema,
        "select row_number() over (partition by a {{ fivetran_utils.partition_by_source_relation() }} order by b) as rn from t",
    ));
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        schema,
        "select a, count(*) as n from t\n{{ dbt_utils.group_by(1) }}",
    ));
}

/// Class dbt-loop-alias: join aliases built in a `{% for %}` loop
/// (`{{ field }}_ordinal`) and referenced by their expanded names (dbt_quickbooks)
#[test]
#[ignore = "known false positive (corpus report: dbt-loop-alias)"]
fn dbt_alias_built_in_for_loop() {
    assert_clean(&dbt(
        SqlDialect::PostgreSQL,
        "CREATE TABLE k (a INT, b INT);\nCREATE TABLE o (a INT, b INT, v INT);",
        "{% set fields = ['a', 'b'] %}\nselect\n  coalesce(a_o.v, b_o.v) as v\nfrom k\n\
         {% for f in fields %}\nleft join o as {{ f }}_o on k.{{ f }} = {{ f }}_o.{{ f }}\n{% endfor %}",
    ));
}

/// Class sf-recursive-cte: Snowflake runs a self-referencing CTE without RECURSIVE
/// (sf-samples PERF-AU-54)
#[test]
fn snowflake_recursive_cte_without_keyword() {
    assert_clean(&check(
        SqlDialect::Snowflake,
        "CREATE TABLE t (id INT);",
        "WITH nums (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM nums WHERE n < 3) SELECT n FROM nums;",
    ));
}

/// Class bq-unpivot: UNPIVOT output keeps the other input columns (bigquery-etl
/// ga_desktop_conversions_v1)
#[test]
#[ignore = "known false positive (corpus report: bq-unpivot)"]
fn bigquery_unpivot_columns() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.t (id INT64, a INT64, b INT64);",
        "SELECT id, val, col FROM ds.t UNPIVOT (val FOR col IN (a, b));",
    ));
}

/// Class bq-groupby-alias: in BigQuery a GROUP BY name matching a SELECT alias is the
/// alias, not an ambiguous input column (bigquery-etl braze_derived/fxa_services_v1)
#[test]
#[ignore = "known false positive (corpus report: bq-groupby-alias)"]
fn bigquery_group_by_output_alias() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.a (uid INT64, service STRING);\nCREATE TABLE ds.e (uid INT64, email STRING);",
        "SELECT a.uid AS uid, MIN(e.email) AS email FROM ds.a AS a JOIN ds.e AS e ON a.uid = e.uid GROUP BY uid;",
    ));
}

/// Class bq-struct-star: `struct_column.*`
#[test]
#[ignore = "known false positive (corpus report: bq-struct-star)"]
fn bigquery_struct_star() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.t (a INT64, info STRUCT<x INT64, y INT64>);",
        "SELECT info.* FROM ds.t;",
    ));
}

/// Class rs-datepart-sysdate: Redshift date parts as bare identifiers and `sysdate`
/// (amazon-redshift-utils AdminScripts)
#[test]
fn redshift_date_part_identifier_and_sysdate() {
    assert_clean(&check(
        SqlDialect::Redshift,
        "CREATE TABLE t (dt TIMESTAMP);",
        "SELECT date_part(h, dt), sysdate - dt FROM t;",
    ));
}

/// Class rs-ddl-attributes: CREATE TABLE with DISTKEY / SORTKEY / DISTSTYLE / ENCODE is
/// skipped, so every query on the table reports E0001 (TPC-H/TPC-DS DDL of
/// amazon-redshift-utils: 3784 diagnostics)
#[test]
fn redshift_table_attributes_in_ddl() {
    assert_clean(&check(
        SqlDialect::Redshift,
        "CREATE TABLE t (id INT8 NOT NULL, v VARCHAR(10) ENCODE zstd) DISTKEY(id) SORTKEY(id);\n\
         CREATE TABLE u (id INT) DISTSTYLE ALL;",
        "SELECT t.id, v FROM t JOIN u ON u.id = t.id;",
    ));
}

/// Class db-types: Databricks MAP / STRUCT<a: INT> columns make the CREATE TABLE skipped,
/// LONG is not numeric, LATERAL VIEW column aliases are not known
#[test]
fn databricks_complex_types_and_lateral_view() {
    assert_clean(&check(
        SqlDialect::Databricks,
        "CREATE TABLE t (id INT, n LONG, tags ARRAY<STRING>, m MAP<STRING, INT>, s STRUCT<a: INT, b: STRING>);",
        "SELECT id, n + 1, m['k'], s.a FROM t;",
    ));
    assert_clean(&check(
        SqlDialect::Databricks,
        "CREATE TABLE t (id INT, tags ARRAY<STRING>);",
        "SELECT id, tag FROM t LATERAL VIEW explode(tags) x AS tag;",
    ));
}

/// Class custom-type-arith: a type sqlsift doesn't know (PostgreSQL MONEY) is reported
/// as non-numeric in arithmetic instead of being unknown
#[test]
#[ignore = "known false positive (corpus report: custom-type-arith)"]
fn unknown_type_in_arithmetic() {
    assert_clean(&check(
        SqlDialect::PostgreSQL,
        "CREATE TABLE t (m MONEY);",
        "SELECT m * 2 FROM t;",
    ));
}

/// Class bq-script-variables: variables of a BigQuery script (`DECLARE x ...`) used as
/// bare names (bigquery-utils storage_billing_model_savings_ddl.sql)
#[test]
#[ignore = "known false positive (corpus report: bq-script-variables)"]
fn bigquery_script_variables() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.t (a FLOAT64);",
        "DECLARE price FLOAT64 DEFAULT 0.02;\nSELECT a * price AS cost FROM ds.t;",
    ));
}

/// Class bq-using-unaliased-subquery: `SELECT *` over `JOIN (subquery) USING (c)` with
/// an unaliased subquery counts `c` twice (bigquery-etl adm_forecasting_v1)
#[test]
#[ignore = "known false positive (corpus report: bq-using-unaliased-subquery)"]
fn bigquery_star_over_using_with_unaliased_subquery() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.b (client_id STRING, d DATE, country STRING);\nCREATE TABLE ds.e (client_id STRING);",
        "WITH m AS (SELECT client_id FROM ds.e)\n\
         SELECT * FROM m LEFT JOIN (SELECT d, client_id, country FROM ds.b) USING (client_id)\n\
         UNION ALL\nSELECT client_id, d, country FROM ds.b;",
    ));
}

/// Class fn-star-duplicate-columns (false negative): a CTE whose `SELECT *` over a join
/// has two columns with the same name; referencing that name is ambiguous in
/// PostgreSQL. Mutation testing: most missed "ambiguous" mutants
#[test]
#[ignore = "known false negative (corpus report: fn-star-duplicate-columns)"]
fn duplicate_columns_of_a_star_cte_are_ambiguous() {
    let found = check(
        SqlDialect::PostgreSQL,
        "CREATE TABLE t (id INT, v INT);",
        "WITH x AS (SELECT * FROM t CROSS JOIN t AS zz_dup) SELECT id FROM x;",
    );
    assert_eq!(kinds(&found), vec![DiagnosticKind::AmbiguousColumn]);
}

// --- breakages the mutation testing injects are found --------------------------------

const MODEL_SCHEMA: &str =
    "CREATE TABLE raw.orders (id INT, customer_id INT, amount NUMERIC, ordered_at DATE);\n\
                            CREATE TABLE raw.customers (id INT, name TEXT);";

const MODEL: &str = "with orders as (
    select id as order_id, customer_id, amount from {{ source('raw', 'orders') }}
),
customers as (
    select id as customer_id, name from {{ source('raw', 'customers') }}
)
select c.customer_id, c.name, sum(o.amount) as total
from customers c
left join orders o on o.customer_id = c.customer_id
group by c.customer_id, c.name
";

#[test]
fn mutations_of_a_clean_model_are_found() {
    let dialects = [
        SqlDialect::PostgreSQL,
        SqlDialect::Snowflake,
        SqlDialect::BigQuery,
        SqlDialect::Redshift,
        SqlDialect::Databricks,
    ];
    for dialect in dialects {
        assert_clean(&dbt(dialect, MODEL_SCHEMA, MODEL));
        // column-typo in a source table (also makes `o.amount` of the outer query unknown)
        let typo = MODEL.replace("customer_id, amount from", "customer_id, amout from");
        let found = dbt(dialect, MODEL_SCHEMA, &typo);
        assert!(
            found
                .iter()
                .any(|d| d.kind == DiagnosticKind::ColumnNotFound && d.message.contains("'amout'")),
            "{dialect}"
        );
        // cte-typo (wrong table)
        let cte = MODEL.replace("left join orders o", "left join orderz o");
        assert_eq!(
            kinds(&dbt(dialect, MODEL_SCHEMA, &cte)),
            vec![DiagnosticKind::TableNotFound],
            "{dialect}"
        );
        // ambiguous after a self join
        let ambiguous = MODEL.replace(
            "{{ source('raw', 'customers') }}",
            "{{ source('raw', 'customers') }} cross join {{ source('raw', 'customers') }} as zz_dup",
        );
        assert!(
            kinds(&dbt(dialect, MODEL_SCHEMA, &ambiguous))
                .contains(&DiagnosticKind::AmbiguousColumn),
            "{dialect}"
        );
    }
}
