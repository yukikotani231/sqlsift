//! Minimal reproductions distilled from the real-world corpus (`tests/corpus/`):
//! dbt packages (jaffle_shop, fivetran dbt_* packages, dbt-utils), mozilla/bigquery-etl,
//! Snowflake-Labs/sf-samples and awslabs/amazon-redshift-utils, and application
//! repositories (sqlc, Prisma TypedSQL, postgres.js, Supabase and MySQL projects; the
//! tests for those are rewritten with tables and columns of our own).
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

// --- app-developer corpus: sqlc, Prisma TypedSQL, postgres.js, Supabase, MySQL ----------
//
// Minimal rewrites (our own tables and columns) of the patterns the app corpus found.

fn embedded(schema: &str, source: &str) -> Vec<Diagnostic> {
    let mut builder = SchemaBuilder::new();
    builder.parse(schema).unwrap();
    let catalog = builder.build().0;
    Analyzer::new(&catalog).analyze_embedded(source, &["sql"])
}

fn messages(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.kind.code(), d.message))
        .collect()
}

/// goose migrations hold both directions in one file (`-- +goose Up` / `-- +goose
/// Down`), and sqlc reads them as the schema; the Down section drops what Up creates
/// (hatchet, bcc-media-platform)
#[test]
fn goose_down_sections_are_not_schema() {
    let schema = "\
-- +goose Up
-- +goose StatementBegin
CREATE TABLE hooks (owner_id UUID NOT NULL, label TEXT NOT NULL);
-- +goose StatementEnd

-- +goose Down
-- +goose StatementBegin
DROP TABLE hooks;
-- +goose StatementEnd
";
    let query = "SELECT label FROM hooks WHERE owner_id = $1;";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, query));
    // sql-migrate (`-- +migrate Up` / `-- +migrate Down`) is the same convention
    let schema = schema
        .replace("+goose Up", "+migrate Up")
        .replace("+goose Down", "+migrate Down");
    assert_clean(&check(SqlDialect::PostgreSQL, &schema, query));
    let typo = "SELECT labl FROM hooks;";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, &schema, typo)),
        [DiagnosticKind::ColumnNotFound]
    );
}

/// An exclusion constraint (`EXCLUDE USING gist`), which sqlparser doesn't parse, made
/// the table skipped, so every query on it reported E0001 (hatchet)
#[test]
fn postgres_exclusion_constraint_in_create_table() {
    let schema = "
CREATE TABLE bookings (
    room TEXT NOT NULL,
    during TSTZRANGE NOT NULL,
    CONSTRAINT no_overlap EXCLUDE USING gist (during WITH &&)
);";
    let query = "SELECT b.room FROM bookings b WHERE b.during @> NOW();";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, query));
    let typo = "SELECT b.rom FROM bookings b;";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, schema, typo)),
        [DiagnosticKind::ColumnNotFound]
    );
}

/// MySQL: inline indexes with a direction or a prefix length, a named FOREIGN KEY
/// index and `DOUBLE(M,D)` / `FLOAT(M,D)` made sqlparser skip the CREATE TABLE, so every
/// query on the table reported E0001 (nanomdm, ranger-ims-go)
#[test]
fn mysql_create_table_index_and_type_syntax() {
    let schema = "
CREATE TABLE jobs (
    id    VARCHAR(64) NOT NULL,
    step  VARCHAR(64) NOT NULL,
    rank  TINYINT NOT NULL DEFAULT 0,
    added TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id, step),
    INDEX (rank DESC, added),
    KEY by_step (step(16) ASC)
);
create table `NODE` (
    ID     integer      not null auto_increment,
    TITLE  varchar(128) not null,
    PARENT integer,
    SCORE  double(8,2),
    primary key (ID),
    unique key (TITLE),
    foreign key `NODE_PARENT`(PARENT) references `NODE`(ID)
);
CREATE TABLE `shift` (
  `id` int(11) NOT NULL AUTO_INCREMENT,
  `hours` double(8,2) NOT NULL DEFAULT 0.00,
  `rate` float(5,2) DEFAULT NULL,
  PRIMARY KEY (`id`),
  CONSTRAINT `shift_self` FOREIGN KEY shift_self_idx (`id`) REFERENCES `shift` (`id`)
);";
    let queries = "
SELECT id, step FROM jobs AS j ORDER BY j.rank DESC, j.added;
select n.ID, n.SCORE from NODE n where n.TITLE = ?;
select id, hours * rate from shift;";
    assert_clean(&check(SqlDialect::MySQL, schema, queries));
    let typos = "SELECT j.rnk FROM jobs j; select n.TITEL from NODE n; select rte from shift;";
    assert_eq!(
        kinds(&check(SqlDialect::MySQL, schema, typos)),
        [DiagnosticKind::ColumnNotFound; 3]
    );
}

/// MySQL: single-table `UPDATE ... [ORDER BY ...] LIMIT n` (nanomdm)
#[test]
fn mysql_update_with_limit() {
    let schema = "CREATE TABLE gadgets (id VARCHAR(64) NOT NULL PRIMARY KEY, secret TEXT, secret_at TIMESTAMP NULL);";
    let query = "
UPDATE
    gadgets
SET
    secret = ?,
    secret_at = CURRENT_TIMESTAMP
WHERE
    id = ?
LIMIT
    1;
UPDATE gadgets SET secret = NULL ORDER BY secret_at LIMIT 10;";
    assert_clean(&check(SqlDialect::MySQL, schema, query));
    let typo = "UPDATE gadgets SET secrt = ? WHERE id = ? LIMIT 1;";
    assert_eq!(
        kinds(&check(SqlDialect::MySQL, schema, typo)),
        [DiagnosticKind::ColumnNotFound]
    );
}

/// A BEFORE INSERT trigger fills a NOT NULL column (`NEW.created_by := ...`), so an
/// INSERT may omit it (basejump, a Supabase project)
#[test]
fn before_insert_trigger_may_fill_not_null_columns() {
    let schema = "
CREATE SCHEMA app;
CREATE TABLE app.notes (
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    body text NOT NULL,
    created_by uuid NOT NULL,
    PRIMARY KEY (id)
);
CREATE OR REPLACE FUNCTION app.set_note_author()
    RETURNS TRIGGER AS
$$
BEGIN
    NEW.created_by = auth.uid();
    RETURN NEW;
END
$$ LANGUAGE plpgsql;
CREATE TRIGGER set_note_author
    BEFORE INSERT
    ON app.notes
    FOR EACH ROW
EXECUTE FUNCTION app.set_note_author();
CREATE TABLE app.tags (id uuid PRIMARY KEY, label text NOT NULL);
";
    let query = "insert into app.notes (body) values ('hi');";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, query));
    // tables without such a trigger are still checked
    let other = "insert into app.tags (id) values ('00000000-0000-0000-0000-000000000000');";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, schema, other)),
        [DiagnosticKind::MissingRequiredColumn]
    );
}

/// A MySQL BEFORE INSERT trigger (`SET NEW.col = ...`, which sqlparser doesn't parse)
/// may fill NOT NULL columns too
#[test]
fn mysql_before_insert_trigger_may_fill_not_null_columns() {
    let schema = "
CREATE TABLE cards (id CHAR(36) NOT NULL PRIMARY KEY, front VARCHAR(100) NOT NULL);
CREATE TRIGGER cards_id BEFORE INSERT ON cards FOR EACH ROW SET NEW.id = UUID();
CREATE TABLE decks (id CHAR(36) NOT NULL PRIMARY KEY, name VARCHAR(100) NOT NULL);
CREATE TRIGGER decks_count AFTER INSERT ON decks FOR EACH ROW SET @n = @n + 1;";
    assert_clean(&check(
        SqlDialect::MySQL,
        schema,
        "INSERT INTO cards (front) VALUES ('q');",
    ));
    // An AFTER trigger can't
    assert_eq!(
        kinds(&check(
            SqlDialect::MySQL,
            schema,
            "INSERT INTO decks (name) VALUES ('x');"
        )),
        [DiagnosticKind::MissingRequiredColumn]
    );
}

/// postgres.js: a JOIN built by a helper and interpolated in the FROM clause
/// (`${withCategory(sql, 'b')}`) brings an alias into scope (podcst-web)
#[test]
fn postgres_js_fragment_between_clauses_may_add_relations() {
    let schema = "
CREATE TABLE books (id BIGINT PRIMARY KEY, title TEXT, category_id INT);
CREATE TABLE categories (id INT PRIMARY KEY, parent_id INT, label TEXT);";
    let source = "
const rows = await sql`
    SELECT b.id, b.title, c.label AS category, category_label
    FROM books b
    ${withCategory(sql, 'b')}
    WHERE c.id = ${categoryId}
    ORDER BY b.id
`;";
    assert_clean(&embedded(schema, source));
    // columns of the query's own tables are still checked
    let typo = "
const rows = await sql`
    SELECT b.titel FROM books b ${withCategory(sql, 'b')} WHERE c.id = ${categoryId}
`;";
    assert_eq!(
        messages(&embedded(schema, typo)),
        ["E0002: Column 'titel' not found in table 'books'"]
    );
}

/// postgres.js: `sql`DESC`` is an ORDER BY direction fragment, not MySQL's
/// `DESC table`, and a query interpolated where a subquery goes
/// (`FROM (${q}) x`, `WITH f AS (${q})`, `IN (${q} UNION ...)`) is a relation of
/// unknown columns (podcst-web)
#[test]
fn postgres_js_query_fragments() {
    let schema = "CREATE TABLE books (id BIGINT PRIMARY KEY, isbn TEXT, shelf INT);";
    let source = "
const order = asc ? sql`ASC` : sql`DESC`;
const rows = await sql`
    SELECT b.id, ext.rating, b.isbn
    FROM (${externalRatings(sql, ids)}) ext
    JOIN books b ON b.id = ext.id
`;
const due = await sql`
    WITH wanted AS (${sql.unsafe(WANTED_IDS_SQL)}),
    picked AS (SELECT b.id FROM books b JOIN wanted w ON w.id = b.id)
    SELECT id FROM picked
`;
const ids = await sql`
    SELECT b.id FROM books b
    WHERE b.id IN (
      ${idsByIsbn(sql, isbn)}
      UNION SELECT id FROM (${externalRatings(sql, [isbn])}) ext
      UNION SELECT id FROM books WHERE shelf = ${shelf ?? null}::integer
    )
`;";
    assert_clean(&embedded(schema, source));
}

/// sqlc: `SELECT ... FOR NO KEY UPDATE` (simplebank) and `ANALYZE table` without
/// `TABLE` (hatchet)
#[test]
fn postgres_row_lock_strengths_and_analyze() {
    let schema =
        "CREATE TABLE wallets (id BIGSERIAL PRIMARY KEY, holder VARCHAR NOT NULL, amount BIGINT NOT NULL);";
    let queries = "
-- name: GetWalletForUpdate :one
SELECT * FROM wallets
WHERE id = $1 LIMIT 1
FOR NO KEY UPDATE;
SELECT id FROM wallets WHERE holder = $1 FOR KEY SHARE;
ANALYZE wallets;
ANALYZE VERBOSE wallets;";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, queries));
    let typo = "SELECT * FROM wallets WHERE idd = $1 FOR NO KEY UPDATE;";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, schema, typo)),
        [DiagnosticKind::ColumnNotFound]
    );
}

/// Supabase test files start with `CREATE EXTENSION ... WITH VERSION '...'`, which
/// sqlparser doesn't parse: the statement only loads functions and types, so it is
/// skipped instead of reported (basejump)
#[test]
fn postgres_create_extension_with_version() {
    let schema = "CREATE TABLE notes (id SERIAL PRIMARY KEY, body TEXT NOT NULL);";
    let queries = "
BEGIN;
CREATE EXTENSION \"pgtap\" WITH SCHEMA extensions VERSION '1.2.0';
create extension if not exists citext version '1.6';
SELECT body FROM notes WHERE id = 1;
ROLLBACK;";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, queries));
    let typo = "CREATE EXTENSION pgtap VERSION '1.2.0'; SELECT bdy FROM notes;";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, schema, typo)),
        [DiagnosticKind::ColumnNotFound]
    );
}

/// pgAdmin-style DDL: a foreign key's `MATCH SIMPLE ... NOT VALID`, and
/// `ADD COLUMN ... varchar array`, made sqlparser skip the table and the column
/// (bcc-media-platform)
#[test]
fn postgres_match_simple_not_valid_and_array_suffix() {
    let schema = r#"
CREATE SCHEMA crm;
CREATE TABLE crm.accounts (id uuid PRIMARY KEY);
CREATE TABLE IF NOT EXISTS crm.reports
(
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    account_id uuid NOT NULL,
    labels text[] NOT NULL DEFAULT '{}'::text[],
    PRIMARY KEY (id),
    CONSTRAINT fk_account FOREIGN KEY (account_id)
        REFERENCES crm.accounts (id) MATCH SIMPLE
        ON UPDATE NO ACTION
        ON DELETE CASCADE
        NOT VALID
);
CREATE TABLE "crm"."phones" (number text PRIMARY KEY);
ALTER TABLE IF EXISTS "crm"."phones" ADD COLUMN IF NOT EXISTS "locales" varchar array NOT NULL;
"#;
    let queries = "
SELECT * FROM crm.reports WHERE id = $1::uuid;
INSERT INTO crm.phones (number, locales) VALUES ($1, $2);";
    assert_clean(&check(SqlDialect::PostgreSQL, schema, queries));
    let typos = "SELECT label FROM crm.reports; SELECT locale FROM crm.phones;";
    assert_eq!(
        kinds(&check(SqlDialect::PostgreSQL, schema, typos)),
        [DiagnosticKind::ColumnNotFound; 2]
    );
}

// --- known false positives and parse gaps (not fixed yet) ---------------------------

/// Class dbt-type-macro-arith: `cast(x as {{ dbt.type_numeric() }})` is a placeholder
/// type, reported as non-numeric in arithmetic (fivetran stripe, hubspot, shopify,
/// fivetran_log, jira, zendesk)
#[test]
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
fn bigquery_group_by_output_alias() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.a (uid INT64, service STRING);\nCREATE TABLE ds.e (uid INT64, email STRING);",
        "SELECT a.uid AS uid, MIN(e.email) AS email FROM ds.a AS a JOIN ds.e AS e ON a.uid = e.uid GROUP BY uid;",
    ));
}

/// Class bq-struct-star: `struct_column.*`
#[test]
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
fn bigquery_script_variables() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.t (a FLOAT64);",
        "DECLARE price FLOAT64 DEFAULT 0.02;\nSELECT a * price AS cost FROM ds.t;",
    ));
}

/// Parse gaps (bigquery-etl, bigquery-utils): `ANY TYPE` parameters, `<<`, field
/// access and `.*` on function results, typed `ARRAY<STRUCT<...>>[...]` literals
#[test]
fn bigquery_syntax_sqlparser_does_not_parse() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE TABLE ds.t (a INT64, v STRING);",
        "CREATE TEMP FUNCTION f(x ANY TYPE) AS (x);\n\
         SELECT 1 << 0 AS m, mozfun.norm.browser_version_info(v).major_version AS major,\n\
         IF(a > 0, f(v), NULL).* REPLACE (1 AS b),\n\
         ARRAY<STRUCT<k STRING, n INT64>>[('a', 1)] AS kv FROM ds.t;",
    ));
}

/// Class harness-bq-project: `moz-fx-data-bq-fivetran.google_ads.x` is not the
/// `google_ads.x` of the project the schema describes
#[test]
fn bigquery_table_of_another_project() {
    assert_clean(&check(
        SqlDialect::BigQuery,
        "CREATE VIEW `moz-fx-data-shared-prod.google_ads.campaigns` AS SELECT 1 AS id;",
        "SELECT campaign_name FROM `moz-fx-data-bq-fivetran.google_ads.campaigns`;",
    ));
}

/// Class bq-using-unaliased-subquery: `SELECT *` over `JOIN (subquery) USING (c)` with
/// an unaliased subquery counts `c` twice (bigquery-etl adm_forecasting_v1)
#[test]
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
