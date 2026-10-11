//! Snowflake dialect (alpha): types, functions, FLATTEN, wildcard options and
//! identifier case, on dbt-style models

use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::types::SqlType;
use sqlsift_core::{Analyzer, Diagnostic, DiagnosticKind, SqlDialect};

const SCHEMA: &str = r#"
CREATE TABLE raw_orders (
    id NUMBER(38, 0) NOT NULL,
    customer_id NUMBER,
    amount NUMBER(12, 2),
    discount_rate FLOAT,
    status VARCHAR(20),
    channel STRING,
    note TEXT,
    is_paid BOOLEAN,
    order_date DATE,
    created_at TIMESTAMP_NTZ,
    updated_at TIMESTAMP_LTZ(9),
    shipped_at TIMESTAMP_TZ,
    payload VARIANT,
    attributes OBJECT,
    items ARRAY,
    signature BINARY,
    delivery_location GEOGRAPHY,
    "camelCaseCol" INTEGER
);
CREATE TABLE raw_customers (
    id NUMBER(38, 0) NOT NULL,
    first_name VARCHAR,
    last_name VARCHAR,
    email VARCHAR(256),
    signup_ts TIMESTAMP_NTZ
);
CREATE TABLE raw_events (
    event_id VARCHAR NOT NULL,
    user_id NUMBER,
    event_ts TIMESTAMP_NTZ,
    properties VARIANT
);
"#;

fn catalog() -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::Snowflake);
    builder.parse(SCHEMA).unwrap();
    let (catalog, warnings) = builder.build();
    assert!(warnings.is_empty(), "{warnings:?}");
    catalog
}

fn check(sql: &str) -> Vec<Diagnostic> {
    Analyzer::with_dialect(&catalog(), SqlDialect::Snowflake).analyze(sql)
}

fn kinds(sql: &str) -> Vec<DiagnosticKind> {
    check(sql).into_iter().map(|d| d.kind).collect()
}

#[track_caller]
fn assert_clean(sql: &str) {
    let diagnostics = check(sql);
    assert!(diagnostics.is_empty(), "{sql}\n{diagnostics:#?}");
}

fn column_type(table: &str, column: &str) -> SqlType {
    catalog()
        .get_table(&QualifiedName::new(table))
        .and_then(|t| t.get_column(column))
        .map(|c| c.data_type.clone())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[test]
fn snowflake_types_map_to_sql_types() {
    assert_eq!(
        column_type("raw_orders", "id"),
        SqlType::Decimal {
            precision: Some(38),
            scale: Some(0)
        }
    );
    assert_eq!(
        column_type("raw_orders", "customer_id"),
        SqlType::Decimal {
            precision: None,
            scale: None
        }
    );
    assert_eq!(
        column_type("raw_orders", "amount").display_name(),
        "numeric(12,2)"
    );
    assert_eq!(
        column_type("raw_orders", "discount_rate"),
        SqlType::DoublePrecision
    );
    assert_eq!(column_type("raw_orders", "channel"), SqlType::Text);
    assert_eq!(column_type("raw_orders", "is_paid"), SqlType::Boolean);
    assert_eq!(
        column_type("raw_orders", "created_at"),
        SqlType::Timestamp {
            precision: None,
            with_timezone: false
        }
    );
    assert_eq!(
        column_type("raw_orders", "updated_at"),
        SqlType::Timestamp {
            precision: Some(9),
            with_timezone: true
        }
    );
    assert_eq!(
        column_type("raw_orders", "shipped_at").display_name(),
        "timestamp with time zone"
    );
    assert_eq!(column_type("raw_orders", "signature"), SqlType::Bytea);
    for (column, name) in [
        ("payload", "variant"),
        ("attributes", "object"),
        ("items", "array"),
        ("delivery_location", "geography"),
    ] {
        let ty = column_type("raw_orders", column);
        assert_eq!(ty, SqlType::SemiStructured(name.into()), "{column}");
        assert_eq!(ty.display_name(), name);
    }
}

#[test]
fn semi_structured_values_are_never_type_checked() {
    assert_clean(
        "SELECT id FROM raw_orders
         WHERE payload = 'x' OR payload:status = 'paid' OR payload:total > 10
            OR payload['k'] = 1 OR payload:items[0].sku = 'abc'
            OR attributes:color = 'red' OR items = 1 OR delivery_location = 'POINT(1 1)'
            OR payload + 1 > 2 OR payload:amount * 2 > 10",
    );
    // Casts out of VARIANT are typed again
    assert_eq!(
        kinds("SELECT id FROM raw_orders WHERE payload:total::number = 'abc'"),
        vec![DiagnosticKind::TypeMismatch]
    );
    // INSERT / UPDATE of semi-structured columns accept anything
    assert_clean("INSERT INTO raw_orders (id, payload, items) VALUES (1, 'x', 2)");
    assert_clean("UPDATE raw_orders SET payload = PARSE_JSON(note), attributes = 1");
}

#[test]
fn number_columns_are_checked_against_string_literals() {
    assert_eq!(
        kinds("SELECT id FROM raw_orders WHERE amount = 'abc'"),
        vec![DiagnosticKind::TypeMismatch]
    );
    assert_eq!(
        kinds("SELECT id FROM raw_orders WHERE customer_id + 'x' > 0"),
        vec![DiagnosticKind::TypeMismatch]
    );
    assert_eq!(
        kinds("INSERT INTO raw_orders (id, amount) VALUES (1, 'lots')"),
        vec![DiagnosticKind::TypeMismatch]
    );
    // Numeric strings and dates in strings convert
    assert_clean(
        "SELECT id FROM raw_orders
         WHERE amount > '1.5' AND id = '42' AND order_date >= '2024-01-01'
           AND created_at < '2024-01-01 00:00:00' AND is_paid = 'true'",
    );
}

#[test]
fn snowflake_coerces_string_columns_implicitly() {
    // Snowflake casts a VARCHAR to a number / date / boolean at run time
    assert_clean(
        "SELECT o.id FROM raw_orders o JOIN raw_events e ON e.event_id = o.id
         WHERE o.status = o.amount AND o.note + 1 > 0 AND o.channel = o.order_date
           AND o.is_paid = 1",
    );
    // Other dialects still report it
    let mut builder = SchemaBuilder::new();
    builder
        .parse("CREATE TABLE t (n INTEGER, s TEXT);")
        .unwrap();
    let pg = builder.build().0;
    assert_eq!(
        Analyzer::new(&pg)
            .analyze("SELECT n FROM t WHERE n = s")
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[test]
fn snowflake_function_return_types() {
    let mismatches = [
        "IFF(is_paid, 1, 2)",
        "NVL(amount, 0)",
        "NVL2(amount, 1, 0)",
        "ZEROIFNULL(amount)",
        "NULLIFZERO(amount)",
        "DECODE(status, 'a', 1, 'b', 2, 0)",
        "DIV0(amount, customer_id)",
        "TRY_TO_NUMBER(note)",
        "DATEDIFF(day, order_date, created_at)",
        "DATEDIFF('day', order_date, created_at)",
        "COUNT_IF(is_paid)",
        "HASH(id)",
        "LEN(note)",
        "REGEXP_COUNT(note, 'a')",
        "YEAR(order_date)",
        "DATE_PART(epoch_second, created_at)",
        "ANY_VALUE(amount)",
    ];
    for expr in mismatches {
        let sql = format!("SELECT id FROM raw_orders WHERE {expr} = 'not a number'");
        assert_eq!(
            kinds(&sql),
            vec![DiagnosticKind::TypeMismatch],
            "{expr} should be numeric"
        );
    }
    let booleans = [
        "BOOLOR_AGG(is_paid)",
        "BOOLAND_AGG(is_paid)",
        "REGEXP_LIKE(note, 'a.*')",
        "STARTSWITH(note, 'a')",
        "TO_BOOLEAN(note)",
    ];
    for expr in booleans {
        let sql = format!("SELECT id FROM raw_orders WHERE {expr} = 'maybe'");
        assert_eq!(
            kinds(&sql),
            vec![DiagnosticKind::TypeMismatch],
            "{expr} should be boolean"
        );
    }
    // Values of the right type, and functions with semi-structured or unknown results
    assert_clean(
        "SELECT
             IFF(amount > 100, 'large', 'small') AS size,
             NVL(channel, 'web') AS channel,
             DECODE(status, 'p', 'paid', 'u', 'unpaid', 'other') AS status_label,
             SPLIT_PART(email_like, '@', 2) AS domain,
             LISTAGG(note, ', ') WITHIN GROUP (ORDER BY created_at) AS notes,
             ARRAY_AGG(id) AS ids,
             OBJECT_CONSTRUCT('id', id, 'amount', amount) AS obj,
             PARSE_JSON(note):a AS parsed,
             MD5(note) AS h,
             UUID_STRING() AS uid,
             REGEXP_REPLACE(note, '[0-9]', '') AS cleaned,
             CONVERT_TIMEZONE('UTC', 'America/New_York', created_at) AS local_ts,
             TO_TIMESTAMP_NTZ(note) AS parsed_ts,
             TRY_CAST(note AS NUMBER(10, 2)) AS note_num,
             SOME_UDF(id) AS custom
         FROM raw_orders, (SELECT 'a@b.c' AS email_like)
         WHERE TO_DATE(created_at) >= '2024-01-01'
           AND DATEADD(day, -7, CURRENT_DATE()) <= order_date
           AND DATE_TRUNC('month', created_at) = DATE_TRUNC(month, updated_at)
           AND SOME_UDF(id) = 'anything'
           AND PARSE_JSON(note) = 1
         GROUP BY ALL",
    );
}

#[test]
fn date_part_keywords_are_not_columns() {
    assert_clean(
        "SELECT
             DATEADD(day, 1, order_date),
             DATEADD(dd, -1, created_at),
             DATEADD(MONTH, 3, order_date),
             DATEADD('hour', 2, created_at),
             TIMEADD(minute, 5, created_at),
             TIMESTAMPADD(hrs, 1, created_at),
             DATEDIFF(days, order_date, created_at),
             DATEDIFF(week, signup_ts, created_at),
             TIMESTAMPDIFF(second, created_at, updated_at),
             DATE_TRUNC(quarter, created_at),
             DATE_PART(dayofweek, order_date),
             DATE_PART(epoch, created_at),
             LAST_DAY(order_date, month),
             EXTRACT(epoch_second FROM created_at)
         FROM raw_orders o JOIN raw_customers c ON c.id = o.customer_id",
    );
    // A misspelled date part is still reported, and so are real column typos
    assert_eq!(
        kinds("SELECT DATEADD(dya, 1, order_date) FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_eq!(
        kinds("SELECT DATEADD(day, 1, ordr_date) FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

// ---------------------------------------------------------------------------
// Table functions
// ---------------------------------------------------------------------------

#[test]
fn lateral_flatten_exposes_its_columns() {
    assert_clean(
        "SELECT
             o.id AS order_id,
             f.seq, f.key, f.path, f.index,
             f.value:sku::varchar AS sku,
             f.value:qty::number AS quantity,
             f.this
         FROM raw_orders o,
              LATERAL FLATTEN(input => o.items) f
         WHERE f.index < 10",
    );
    // Unaliased FLATTEN: its columns are visible unqualified
    assert_clean(
        "SELECT o.id, value:sku::string AS sku, index FROM raw_orders o, LATERAL FLATTEN(input => o.items)",
    );
    // TABLE(FLATTEN(...)) and nested FLATTEN over a VARIANT path, with outer join
    assert_clean(
        "SELECT e.event_id, p.key AS prop, p.value AS prop_value
         FROM raw_events e,
              TABLE(FLATTEN(input => e.properties:attributes, outer => true)) p",
    );
    assert_clean(
        "SELECT e.event_id, i.value
         FROM raw_events e,
              LATERAL FLATTEN(input => e.properties:items) i,
              LATERAL FLATTEN(input => i.value:tags) t
         WHERE t.value::string = 'promo'",
    );
    // Columns FLATTEN doesn't have
    assert_eq!(
        kinds("SELECT o.id, f.val FROM raw_orders o, LATERAL FLATTEN(input => o.items) f"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // FLATTEN's input is resolved
    assert_eq!(
        kinds("SELECT f.value FROM raw_orders o, LATERAL FLATTEN(input => o.itemz) f"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn other_table_functions_have_unknown_columns() {
    assert_clean(
        "SELECT SEQ4() AS n, UNIFORM(1, 10, RANDOM()) AS r, g.anything
         FROM TABLE(GENERATOR(ROWCOUNT => 100)) g",
    );
    assert_clean(
        "SELECT o.id, s.value, s.index FROM raw_orders o, TABLE(SPLIT_TO_TABLE(o.note, ',')) s",
    );
    assert_clean("SELECT * FROM TABLE(RESULT_SCAN(LAST_QUERY_ID()))");
    assert_eq!(
        kinds("SELECT s.value FROM raw_orders o, TABLE(SPLIT_TO_TABLE(o.nte, ',')) s"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn identifier_function_names_a_table() {
    assert_clean("SELECT id, amount FROM IDENTIFIER('raw_orders')");
    assert_clean("SELECT raw_orders.id, o.amount FROM IDENTIFIER('raw_orders') JOIN IDENTIFIER('RAW_ORDERS') o ON o.id = raw_orders.id");
    assert_eq!(
        kinds("SELECT amont FROM IDENTIFIER('raw_orders')"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // Tables sqlsift can't find (or session variables) are unknown, never reported
    assert_clean("SELECT anything FROM IDENTIFIER('other_db.other_schema.t')");
    assert_clean("SELECT anything FROM IDENTIFIER($table_name)");
}

// ---------------------------------------------------------------------------
// SELECT * options, GROUP BY ALL, positional columns
// ---------------------------------------------------------------------------

#[test]
fn select_star_exclude_rename_replace() {
    assert_clean(
        "WITH orders AS (
             SELECT * EXCLUDE (payload, attributes) RENAME (amount AS order_total)
             FROM raw_orders
         )
         SELECT id, order_total, status FROM orders",
    );
    assert_eq!(
        kinds(
            "WITH orders AS (SELECT * EXCLUDE (payload) FROM raw_orders)
             SELECT payload FROM orders"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_eq!(
        kinds(
            "WITH orders AS (SELECT * RENAME (amount AS order_total) FROM raw_orders)
             SELECT amount FROM orders"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // REPLACE keeps the column, with the new expression's type
    assert_clean(
        "WITH orders AS (SELECT * REPLACE (ROUND(amount, 0) AS amount) FROM raw_orders)
         SELECT amount FROM orders WHERE amount > 10",
    );
    assert_eq!(
        kinds(
            "WITH orders AS (SELECT * REPLACE (TO_DATE(created_at) AS amount) FROM raw_orders)
             SELECT id FROM orders WHERE amount = 1"
        ),
        vec![DiagnosticKind::TypeMismatch]
    );
    // Qualified wildcards with options
    assert_clean(
        "SELECT o.* EXCLUDE (payload), c.first_name
         FROM raw_orders o JOIN raw_customers c ON c.id = o.customer_id",
    );
    // A column the wildcard's tables don't have
    assert_eq!(
        kinds("SELECT * EXCLUDE (amont) FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_eq!(
        kinds(
            "SELECT o.* EXCLUDE (paylod) FROM raw_orders o JOIN raw_customers c ON c.id = o.customer_id"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // Excluding a column several tables have is fine
    assert_clean(
        "SELECT * EXCLUDE (id) FROM raw_orders o JOIN raw_customers c ON c.id = o.customer_id",
    );
}

#[test]
fn group_by_all_and_positional_columns() {
    assert_clean(
        "SELECT customer_id, DATE_TRUNC('day', created_at) AS order_day, SUM(amount) AS revenue
         FROM raw_orders GROUP BY ALL",
    );
    assert_clean("SELECT $1, $2 FROM raw_orders");
}

// ---------------------------------------------------------------------------
// Identifier case
// ---------------------------------------------------------------------------

#[test]
fn identifier_case() {
    // Unquoted identifiers fold to upper case and match the catalog case-insensitively
    assert_clean("SELECT ID, Amount, CREATED_AT FROM RAW_ORDERS");
    assert_clean("SELECT id FROM \"RAW_ORDERS\" WHERE \"STATUS\" = 'paid'");
    // Quoted names are case-sensitive in Snowflake, but sqlsift can't tell how the
    // schema was created (dbt quoting, ...), so they match permissively
    assert_clean("SELECT \"camelCaseCol\", \"id\" FROM \"raw_orders\"");
    assert_clean("SELECT camelcasecol FROM raw_orders");
    // A name that matches nothing is still reported
    assert_eq!(
        kinds("SELECT \"camelCaseCo\" FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}

#[test]
fn unqualified_typo_is_not_a_lateral_alias_of_itself() {
    // Lateral column aliases only cover explicit aliases of earlier items
    assert_eq!(
        kinds("SELECT amont FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_eq!(
        kinds("SELECT amont AS amont FROM raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
    assert_clean("SELECT amount AS a, a * 2 AS b FROM raw_orders WHERE b > 1");
}

// ---------------------------------------------------------------------------
// dbt-style models
// ---------------------------------------------------------------------------

#[test]
fn dbt_staging_model_is_clean() {
    assert_clean(
        r#"
with source as (
    select * from raw_orders
),

renamed as (
    select
        id as order_id,
        customer_id,
        amount::number(12, 2) as amount_usd,
        zeroifnull(discount_rate) as discount_rate,
        lower(trim(status)) as status,
        nvl(channel, 'unknown') as channel,
        iff(is_paid, 'paid', 'unpaid') as payment_state,
        payload:shipping:carrier::string as carrier,
        payload:shipping:tracking_number::varchar as tracking_number,
        try_to_number(payload:discount_code_value::string) as discount_value,
        to_date(created_at) as order_date,
        convert_timezone('UTC', created_at)::timestamp_ntz as created_at_utc,
        date_trunc('week', created_at) as order_week,
        datediff(day, created_at, shipped_at) as days_to_ship,
        "camelCaseCol" as camel_case_col
    from source
    where created_at >= dateadd(year, -2, current_timestamp())
)

select * from renamed
"#,
    );
}

#[test]
fn dbt_intermediate_model_with_flatten_is_clean() {
    assert_clean(
        r"
with orders as (
    select * exclude (payload, attributes) from raw_orders
),

order_items as (
    select
        o.id as order_id,
        item.index as line_number,
        item.value:sku::string as sku,
        item.value:quantity::number as quantity,
        item.value:unit_price::number(10, 2) as unit_price
    from raw_orders o,
        lateral flatten(input => o.items) item
),

aggregated as (
    select
        order_id,
        count(*) as line_count,
        sum(quantity * unit_price) as items_total,
        array_agg(sku) within group (order by line_number) as skus,
        listagg(distinct sku, ',') as sku_list,
        count_if(quantity > 1) as multi_quantity_lines
    from order_items
    group by all
)

select
    orders.id as order_id,
    orders.customer_id,
    orders.amount,
    aggregated.line_count,
    aggregated.items_total,
    div0(aggregated.items_total, aggregated.line_count) as avg_line_value,
    aggregated.skus,
    aggregated.sku_list,
    abs(orders.amount - aggregated.items_total) < 0.01 as totals_match
from orders
left join aggregated on aggregated.order_id = orders.id
qualify row_number() over (partition by orders.id order by orders.updated_at desc) = 1
",
    );
}

#[test]
fn dbt_mart_model_is_clean() {
    assert_clean(
        r"
with customers as (
    select
        id as customer_id,
        initcap(first_name) || ' ' || initcap(last_name) as full_name,
        split_part(email, '@', 2) as email_domain,
        md5(lower(email)) as email_hash,
        signup_ts
    from raw_customers
),

orders as (
    select
        customer_id,
        min(created_at) as first_order_at,
        max(created_at) as last_order_at,
        count(distinct id) as order_count,
        sum(zeroifnull(amount)) as lifetime_value,
        boolor_agg(is_paid) as has_paid_order,
        any_value(channel) as some_channel
    from raw_orders
    group by customer_id
),

events as (
    select
        user_id,
        count_if(properties:type::string = 'page_view') as page_views,
        max(event_ts) as last_seen_at
    from raw_events
    where event_ts > dateadd(day, -90, current_date())
    group by 1
)

select
    c.customer_id,
    c.full_name,
    c.email_domain,
    c.email_hash,
    o.first_order_at,
    o.last_order_at,
    coalesce(o.order_count, 0) as order_count,
    coalesce(o.lifetime_value, 0) as lifetime_value,
    nvl(o.has_paid_order, false) as has_paid_order,
    e.page_views,
    datediff('day', c.signup_ts, o.first_order_at) as days_to_first_order,
    case
        when o.lifetime_value >= 1000 then 'vip'
        when o.lifetime_value >= 100 then 'regular'
        else 'new'
    end as segment,
    decode(o.some_channel, 'web', 1, 'app', 2, 0) as channel_code,
    uuid_string() as row_uuid,
    hash(c.customer_id, o.first_order_at) as row_hash
from customers c
left join orders o on o.customer_id = c.customer_id
left join events e on e.user_id = c.customer_id
where c.email_domain not in ('example.com', 'test.com')
",
    );
}

#[test]
fn dbt_model_mistakes_are_reported() {
    // Typo'd column
    assert_eq!(
        kinds(
            "with source as (select * from raw_orders)
             select id, custmer_id from source"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // Missing table
    assert_eq!(
        kinds("select id from raw_order"),
        vec![DiagnosticKind::TableNotFound]
    );
    // NUMBER compared with a non-numeric string
    assert_eq!(
        kinds("select id from raw_orders where amount > 'n/a'"),
        vec![DiagnosticKind::TypeMismatch]
    );
    // Column of a CTE that excluded it
    assert_eq!(
        kinds(
            "with o as (select * exclude (status) from raw_orders)
             select id from o where status = 'paid'"
        ),
        vec![DiagnosticKind::ColumnNotFound]
    );
    // Typo in a FLATTEN column and in a joined table's column
    assert_eq!(
        kinds(
            "select o.id, f.vlaue from raw_orders o, lateral flatten(input => o.items) f
             join raw_customers c on c.id = o.customer_id
             where c.emial is not null"
        ),
        vec![
            DiagnosticKind::ColumnNotFound,
            DiagnosticKind::ColumnNotFound
        ]
    );
    // Typo'd column inside a date function
    assert_eq!(
        kinds("select dateadd(day, 1, creatd_at) from raw_orders"),
        vec![DiagnosticKind::ColumnNotFound]
    );
}
