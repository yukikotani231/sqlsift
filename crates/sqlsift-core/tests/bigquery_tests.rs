//! BigQuery dialect (alpha): types, project/dataset names, UNNEST, STRUCT field
//! paths, `SELECT * EXCEPT / REPLACE`, wildcard tables and BigQuery functions

use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::{Analyzer, Diagnostic, DiagnosticKind, SqlDialect};

const SCHEMA: &str = r"
CREATE TABLE analytics.orders (
  order_id INT64 NOT NULL,
  customer_id INT64,
  status STRING,
  amount NUMERIC,
  amount_exact BIGNUMERIC(40, 2),
  discount_rate FLOAT64,
  is_paid BOOL,
  payload BYTES,
  order_date DATE,
  created_at TIMESTAMP,
  local_created_at DATETIME,
  pickup_time TIME,
  attributes JSON,
  delivery_location GEOGRAPHY,
  shipping STRUCT<city STRING, zip STRING, geo STRUCT<lat FLOAT64, lng FLOAT64>>,
  items ARRAY<STRUCT<sku STRING, quantity INT64, unit_price NUMERIC>>,
  tags ARRAY<STRING>
)
PARTITION BY DATE(created_at)
CLUSTER BY customer_id
OPTIONS (description = 'One row per order');

CREATE TABLE `my-project.analytics.customers` (
  customer_id INT64 NOT NULL,
  full_name STRING,
  email STRING,
  signed_up_at TIMESTAMP
);

CREATE TABLE analytics.events_20240101 (event_name STRING, user_pseudo_id STRING, event_timestamp INT64);

-- Defined without a dataset
CREATE TABLE products (sku STRING NOT NULL, product_name STRING, category STRING);
";

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::BigQuery);
    builder.parse(SCHEMA).expect("schema parses");
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "{warnings:?}");
    catalog
}

fn check(sql: &str) -> Vec<Diagnostic> {
    Analyzer::with_dialect(&catalog(), SqlDialect::BigQuery).analyze(sql)
}

#[track_caller]
fn assert_clean(sql: &str) {
    let diagnostics = check(sql);
    assert!(diagnostics.is_empty(), "{sql}\n{diagnostics:#?}");
}

#[track_caller]
fn assert_reports(sql: &str, expected: &[DiagnosticKind]) -> Vec<Diagnostic> {
    let diagnostics = check(sql);
    let kinds: Vec<DiagnosticKind> = diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, expected, "{sql}\n{diagnostics:#?}");
    diagnostics
}

fn column_type(catalog: &Catalog, table: &str, column: &str) -> String {
    catalog
        .get_table(&QualifiedName::parse(table))
        .and_then(|t| t.get_column(column))
        .map(|c| c.data_type.display_name_in(SqlDialect::BigQuery))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[test]
fn bigquery_column_types_are_parsed() {
    let catalog = catalog();
    let ty = |column| column_type(&catalog, "analytics.orders", column);
    assert_eq!(ty("order_id"), "INT64");
    assert_eq!(ty("status"), "STRING");
    assert_eq!(ty("amount"), "NUMERIC");
    assert_eq!(ty("amount_exact"), "NUMERIC");
    assert_eq!(ty("discount_rate"), "FLOAT64");
    assert_eq!(ty("is_paid"), "BOOL");
    assert_eq!(ty("payload"), "BYTES");
    assert_eq!(ty("order_date"), "DATE");
    assert_eq!(ty("created_at"), "TIMESTAMP");
    assert_eq!(ty("pickup_time"), "TIME");
    assert_eq!(ty("attributes"), "JSON");
    assert_eq!(ty("delivery_location"), "GEOGRAPHY");
    // Nested STRUCTs closed by `>>` (which sqlparser rejects before a comma)
    assert!(
        ty("shipping").starts_with("STRUCT<city STRING"),
        "{}",
        ty("shipping")
    );
    assert!(
        ty("items").starts_with("ARRAY<STRUCT<sku STRING"),
        "{}",
        ty("items")
    );
    assert_eq!(ty("tags"), "ARRAY<STRING>");
}

#[test]
fn struct_json_and_geography_values_are_never_type_errors() {
    assert_clean(
        "SELECT order_id FROM analytics.orders \
         WHERE shipping = shipping AND attributes = '{}' AND delivery_location IS NOT NULL",
    );
    assert_clean(
        "SELECT SAFE_CAST(status AS INT64) + 1, CAST(amount AS BIGNUMERIC) FROM analytics.orders",
    );
}

#[test]
fn type_mismatches_use_bigquery_type_names() {
    let diagnostics = assert_reports(
        "SELECT order_id FROM analytics.orders WHERE status = 1",
        &[DiagnosticKind::TypeMismatch],
    );
    assert_eq!(
        diagnostics[0].message,
        "Type mismatch: cannot compare STRING with INT64"
    );
    assert_reports(
        "SELECT order_id FROM analytics.orders WHERE amount = 'abc'",
        &[DiagnosticKind::TypeMismatch],
    );
    assert_reports(
        "SELECT order_id FROM analytics.orders WHERE created_at = 20240101",
        &[DiagnosticKind::TypeMismatch],
    );
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

#[test]
fn project_and_dataset_qualified_names() {
    for from in [
        "analytics.orders",
        "`analytics.orders`",
        "`my-project.analytics.orders`",
        "`my-project`.analytics.orders",
        "`my-project`.`analytics`.`orders`",
        "my_project.analytics.orders",
    ] {
        assert_clean(&format!(
            "SELECT o.order_id, o.status FROM {from} AS o WHERE o.amount > 0"
        ));
        assert_reports(
            &format!("SELECT o.order_total FROM {from} AS o"),
            &[DiagnosticKind::ColumnNotFound],
        );
    }
    assert_clean("SELECT c.full_name FROM `my-project.analytics.customers` AS c");
    assert_clean("SELECT analytics.orders.order_id FROM analytics.orders");
}

#[test]
fn unknown_dataset_or_project_is_not_reported() {
    // A dataset the schema doesn't define may simply not have been loaded
    assert_clean("SELECT anything, at_all FROM `other-project.raw_shop.orders`");
    assert_clean("SELECT * FROM staging.stg_payments p WHERE p.amount > 0");
    // Tables defined without a dataset are found with one
    assert_clean("SELECT sku, product_name FROM `my-project.catalog.products`");
    assert_reports(
        "SELECT product_title FROM `my-project.catalog.products`",
        &[DiagnosticKind::ColumnNotFound],
    );
}

#[test]
fn missing_table_in_a_known_dataset_is_reported() {
    assert_reports(
        "SELECT order_id FROM `my-project.analytics.ordrs`",
        &[DiagnosticKind::TableNotFound],
    );
    assert_reports("SELECT 1 FROM ordrs", &[DiagnosticKind::TableNotFound]);
}

#[test]
fn column_names_are_case_insensitive() {
    assert_clean("SELECT ORDER_ID, Status FROM analytics.orders WHERE Is_Paid");
}

// ---------------------------------------------------------------------------
// UNNEST and STRUCT fields
// ---------------------------------------------------------------------------

#[test]
fn unnest_aliases_are_values_with_unknown_fields() {
    assert_clean(
        "SELECT o.order_id, item.sku, item.quantity * item.unit_price AS line_total \
         FROM analytics.orders AS o, UNNEST(o.items) AS item",
    );
    assert_clean(
        "SELECT o.order_id, tag, pos FROM analytics.orders o \
         CROSS JOIN UNNEST(o.tags) AS tag WITH OFFSET AS pos",
    );
    assert_clean(
        "SELECT o.order_id, item.sku FROM analytics.orders o \
         LEFT JOIN UNNEST(o.items) AS item ON TRUE",
    );
    assert_clean(
        "SELECT o.order_id, \
           (SELECT SUM(i.quantity) FROM UNNEST(o.items) AS i) AS units, \
           (SELECT STRING_AGG(t, ',') FROM UNNEST(o.tags) AS t) AS tag_list \
         FROM analytics.orders o",
    );
    assert_clean("SELECT day FROM UNNEST(GENERATE_DATE_ARRAY('2024-01-01', '2024-12-31')) AS day");
}

#[test]
fn unnest_arguments_are_resolved() {
    assert_reports(
        "SELECT item.sku FROM analytics.orders o, UNNEST(o.line_items) AS item",
        &[DiagnosticKind::ColumnNotFound],
    );
    assert_reports(
        "SELECT item.sku FROM analytics.orders o, UNNEST(x.items) AS item",
        &[DiagnosticKind::TableNotFound],
    );
}

#[test]
fn struct_field_paths_are_not_tables() {
    assert_clean(
        "SELECT o.shipping.city, shipping.zip, shipping.geo.lat, o.shipping.geo.lng \
         FROM analytics.orders o WHERE shipping.city = 'Tokyo'",
    );
    assert_clean(
        "SELECT o.items[SAFE_OFFSET(0)].sku, items[OFFSET(0)].quantity FROM analytics.orders o",
    );
    // A misspelled STRUCT column is still a missing column
    assert_reports(
        "SELECT o.shiping.city FROM analytics.orders o",
        &[DiagnosticKind::ColumnNotFound],
    );
    // ... and a name that is neither an alias nor a column a missing alias
    assert_reports(
        "SELECT x.shipping.city FROM analytics.orders o",
        &[DiagnosticKind::TableNotFound],
    );
}

// ---------------------------------------------------------------------------
// SELECT * EXCEPT / REPLACE, wildcard tables
// ---------------------------------------------------------------------------

#[test]
fn select_star_except_replace_columns() {
    assert_clean(
        "WITH base AS (\
           SELECT * EXCEPT (attributes, delivery_location) REPLACE (UPPER(status) AS status) \
           FROM analytics.orders) \
         SELECT order_id, status FROM base",
    );
    // EXCEPT removes the column from the output
    assert_reports(
        "WITH base AS (SELECT * EXCEPT (attributes) FROM analytics.orders) \
         SELECT attributes FROM base",
        &[DiagnosticKind::ColumnNotFound],
    );
    // EXCEPT of a column that doesn't exist is an error in BigQuery
    assert_reports(
        "SELECT * EXCEPT (atributes) FROM analytics.orders",
        &[DiagnosticKind::ColumnNotFound],
    );
    assert_reports(
        "SELECT o.* EXCEPT (atributes) FROM analytics.orders o",
        &[DiagnosticKind::ColumnNotFound],
    );
    // REPLACE gives the column the replacement's type
    assert_reports(
        "WITH base AS (SELECT * REPLACE (CAST(order_id AS STRING) AS order_id) FROM analytics.orders) \
         SELECT 1 FROM base WHERE order_id = 1",
        &[DiagnosticKind::TypeMismatch],
    );
}

#[test]
fn wildcard_tables_and_table_suffix() {
    assert_clean(
        "SELECT event_name, user_pseudo_id FROM `my-project.analytics.events_*` \
         WHERE _TABLE_SUFFIX BETWEEN '20240101' AND '20240131'",
    );
    assert_clean(
        "SELECT e.event_name FROM `analytics.events_2024*` AS e WHERE e._TABLE_SUFFIX > '0101'",
    );
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[test]
fn date_parts_are_not_columns() {
    assert_clean(
        "SELECT \
           DATE_DIFF(CURRENT_DATE(), order_date, DAY) AS age_days, \
           DATE_TRUNC(order_date, MONTH) AS order_month, \
           DATE_TRUNC(order_date, ISOWEEK) AS order_isoweek, \
           DATE_DIFF(order_date, DATE '2024-01-01', WEEK(MONDAY)) AS weeks, \
           TIMESTAMP_TRUNC(created_at, HOUR) AS created_hour, \
           TIMESTAMP_DIFF(CURRENT_TIMESTAMP(), created_at, MINUTE) AS age_minutes, \
           DATETIME_TRUNC(local_created_at, QUARTER), \
           EXTRACT(DAYOFWEEK FROM order_date) AS dow, \
           EXTRACT(ISOYEAR FROM order_date) AS iso_year \
         FROM analytics.orders",
    );
}

#[test]
fn function_return_types_are_compatible_with_their_uses() {
    assert_clean(
        "SELECT order_id FROM analytics.orders \
         WHERE DATE_DIFF(CURRENT_DATE(), order_date, DAY) > 30 \
           AND FORMAT_DATE('%Y-%m', order_date) = '2024-01' \
           AND SAFE_DIVIDE(amount, discount_rate) > 0.5 \
           AND order_date >= PARSE_DATE('%Y%m%d', '20240101') \
           AND order_date >= SAFE.PARSE_DATE('%Y%m%d', status) \
           AND DATE_TRUNC(order_date, MONTH) = DATE '2024-01-01' \
           AND created_at >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 7 DAY) \
           AND order_date > DATE_SUB(CURRENT_DATE(), INTERVAL 1 MONTH) \
           AND local_created_at < CURRENT_DATETIME() \
           AND JSON_VALUE(attributes, '$.channel') = status \
           AND TO_HEX(MD5(status)) = status \
           AND REGEXP_CONTAINS(status, r'^paid') \
           AND REGEXP_EXTRACT(status, r'(\\w+)') = 'paid' \
           AND ARRAY_LENGTH(tags) > 0 \
           AND IFNULL(customer_id, 0) > 0 \
           AND IF(is_paid, amount, 0) > 10 \
           AND FARM_FINGERPRINT(status) > 0 \
           AND GENERATE_UUID() != status \
           AND SPLIT(status, '-')[SAFE_OFFSET(0)] = 'a'",
    );
    assert_clean(
        "SELECT customer_id, \
           COUNTIF(is_paid) AS paid_orders, \
           LOGICAL_OR(is_paid) AS any_paid, \
           LOGICAL_AND(is_paid) AS all_paid, \
           ANY_VALUE(status) AS some_status, \
           APPROX_COUNT_DISTINCT(order_id) AS approx_orders, \
           ARRAY_AGG(order_id ORDER BY created_at DESC LIMIT 10) AS recent_orders, \
           ARRAY_AGG(STRUCT(order_id, amount) ORDER BY created_at DESC LIMIT 1)[OFFSET(0)].amount AS last_amount, \
           STRING_AGG(status, ', ' ORDER BY created_at) AS statuses \
         FROM analytics.orders \
         GROUP BY customer_id \
         HAVING COUNTIF(is_paid) > 1 AND paid_orders < 1000",
    );
}

#[test]
fn function_return_types_catch_mistakes() {
    for sql in [
        "SELECT 1 FROM analytics.orders WHERE DATE_DIFF(CURRENT_DATE(), order_date, DAY) = 'many'",
        "SELECT 1 FROM analytics.orders WHERE FORMAT_DATE('%Y', order_date) = 2024",
        "SELECT 1 FROM analytics.orders WHERE COUNTIF(is_paid) = 'yes'",
        "SELECT 1 FROM analytics.orders WHERE MD5(status) = status",
        "SELECT 1 FROM analytics.orders WHERE SAFE_CAST(status AS INT64) = 'x'",
        "SELECT 1 FROM analytics.orders WHERE PARSE_DATE('%Y%m%d', status) = 20240101",
    ] {
        assert_reports(sql, &[DiagnosticKind::TypeMismatch]);
    }
}

// ---------------------------------------------------------------------------
// dbt-style models
// ---------------------------------------------------------------------------

/// A compiled dbt staging + mart model, as `dbt compile` writes it
const DBT_MODEL: &str = r"
with source as (
    select * from `my-project`.`analytics`.`orders`
    where _partitiontime >= timestamp_sub(current_timestamp(), interval 3 day)
),

customers as (
    select * from `my-project`.`analytics`.`customers`
),

renamed as (
    select
        order_id,
        customer_id,
        lower(status) as order_status,
        safe_cast(amount as numeric) as amount,
        ifnull(discount_rate, 0) as discount_rate,
        coalesce(is_paid, false) as is_paid,
        order_date,
        timestamp_trunc(created_at, day) as created_day,
        date_trunc(order_date, month) as order_month,
        shipping.city as shipping_city,
        shipping.geo.lat as shipping_lat,
        json_value(attributes, '$.channel') as channel,
        array_length(items) as item_count,
        to_hex(md5(cast(order_id as string))) as order_key
    from source
),

line_items as (
    select
        r.order_id,
        item.sku,
        item.quantity,
        item.unit_price * item.quantity as line_amount,
        item_index
    from source as r
    cross join unnest(r.items) as item with offset as item_index
),

order_totals as (
    select
        order_id,
        sum(line_amount) as items_amount,
        count(distinct sku) as distinct_skus
    from line_items
    group by order_id
)

select
    renamed.* except (discount_rate),
    c.full_name as customer_name,
    order_totals.items_amount,
    order_totals.distinct_skus,
    safe_divide(order_totals.items_amount, renamed.amount) as items_ratio,
    date_diff(current_date(), renamed.order_date, day) as days_since_order,
    format_date('%Y-%m', renamed.order_date) as order_month_label,
    row_number() over (partition by renamed.customer_id order by renamed.order_date) as order_seq
from renamed
left join customers as c using (customer_id)
left join order_totals using (order_id)
where renamed.is_paid
qualify order_seq <= 100
";

#[test]
fn compiled_dbt_model_is_clean() {
    assert_clean(DBT_MODEL);
}

#[test]
fn mistakes_in_a_dbt_model_are_reported() {
    // A column of the source that doesn't exist
    assert_reports(
        &DBT_MODEL.replace(
            "lower(status) as order_status",
            "lower(order_state) as order_status",
        ),
        &[DiagnosticKind::ColumnNotFound],
    );
    // A column the CTE doesn't select
    assert_reports(
        &DBT_MODEL.replace(
            "order_totals.distinct_skus,",
            "order_totals.distinct_products,",
        ),
        &[DiagnosticKind::ColumnNotFound],
    );
    // A misspelled STRUCT column: neither a column nor a table alias
    assert_reports(
        &DBT_MODEL.replace(
            "shipping.city as shipping_city",
            "shiping.city as shipping_city",
        ),
        &[DiagnosticKind::TableNotFound],
    );
    // A misspelled CTE
    assert_reports(
        &DBT_MODEL.replace("from line_items", "from line_item"),
        &[DiagnosticKind::TableNotFound],
    );
    // Comparing a string with a number
    assert_reports(
        &DBT_MODEL.replace("where renamed.is_paid", "where renamed.order_status = 1"),
        &[DiagnosticKind::TypeMismatch],
    );
}
