//! Schema building (DDL -> Catalog) and diagnostic quality tests.
//!
//! The first half verifies that PostgreSQL DDL (CREATE TABLE variants, migration
//! sequences, views and pg_dump-style resilience) produces the expected `Catalog`,
//! both through the public catalog API and by analyzing queries against it.
//!
//! The second half verifies diagnostic quality: message text, help text and spans
//! (1-indexed `line` / `column` in characters, `length`) for every diagnostic code,
//! inline suppression directives and deterministic ordering.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::error::{Diagnostic, DiagnosticKind, Severity};
use sqlsift_core::schema::{
    Catalog, DefaultValue, IdentityKind, QualifiedName, SchemaBuilder, TableDef,
};
use sqlsift_core::types::SqlType;

// =====================================================================
// Helpers
// =====================================================================

/// Build a catalog from a single schema string, asserting there are no warnings.
fn catalog(schema: &str) -> Catalog {
    let (catalog, warnings) = catalog_with_warnings(schema);
    assert!(
        warnings.is_empty(),
        "expected no schema warnings, got: {:?}",
        warnings
    );
    catalog
}

/// Build a catalog from a single schema string, returning warnings too.
fn catalog_with_warnings(schema: &str) -> (Catalog, Vec<Diagnostic>) {
    let mut builder = SchemaBuilder::new();
    let result = builder.parse(schema);
    assert!(
        result.is_ok(),
        "schema parse returned errors: {:?}",
        result.err()
    );
    builder.build()
}

/// Build a catalog by applying several schema "files" in order.
fn catalog_from_files(files: &[&str]) -> (Catalog, Vec<Diagnostic>) {
    let mut builder = SchemaBuilder::new();
    for (i, file) in files.iter().enumerate() {
        let result = builder.parse(file);
        assert!(result.is_ok(), "file #{} returned errors: {:?}", i, result);
    }
    builder.build()
}

fn analyze(catalog: &Catalog, sql: &str) -> Vec<Diagnostic> {
    Analyzer::new(catalog).analyze(sql)
}

#[track_caller]
fn assert_clean(catalog: &Catalog, sql: &str) {
    let diags = analyze(catalog, sql);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for {:?}, got: {:#?}",
        sql,
        diags
    );
}

fn codes(diags: &[Diagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.code()).collect()
}

#[track_caller]
fn assert_codes(catalog: &Catalog, sql: &str, expected: &[&str]) -> Vec<Diagnostic> {
    let diags = analyze(catalog, sql);
    assert_eq!(
        codes(&diags),
        expected,
        "unexpected diagnostic codes for {:?}: {:#?}",
        sql,
        diags
    );
    diags
}

/// Analyze and expect exactly one diagnostic; return it.
#[track_caller]
fn single(catalog: &Catalog, sql: &str) -> Diagnostic {
    let mut diags = analyze(catalog, sql);
    assert_eq!(
        diags.len(),
        1,
        "expected exactly one diagnostic for {:?}, got: {:#?}",
        sql,
        diags
    );
    diags.remove(0)
}

/// (line, column, length) of a diagnostic's span.
#[track_caller]
fn loc(d: &Diagnostic) -> (usize, usize, usize) {
    let span = d
        .span
        .unwrap_or_else(|| panic!("diagnostic has no span: {:?}", d));
    (span.line, span.column, span.length)
}

#[track_caller]
fn table<'a>(catalog: &'a Catalog, name: &str) -> &'a TableDef {
    catalog
        .get_table(&QualifiedName::parse(name))
        .unwrap_or_else(|| {
            panic!(
                "table {:?} missing from catalog; tables = {:?}",
                name,
                catalog.table_names()
            )
        })
}

#[track_caller]
fn col_type(catalog: &Catalog, tbl: &str, col: &str) -> SqlType {
    table(catalog, tbl)
        .get_column(col)
        .unwrap_or_else(|| panic!("column {}.{} missing", tbl, col))
        .data_type
        .clone()
}

#[track_caller]
fn nullable(catalog: &Catalog, tbl: &str, col: &str) -> bool {
    table(catalog, tbl)
        .get_column(col)
        .unwrap_or_else(|| panic!("column {}.{} missing", tbl, col))
        .nullable
}

fn has_table(catalog: &Catalog, name: &str) -> bool {
    catalog.table_exists(&QualifiedName::parse(name))
}

#[track_caller]
fn view_columns(catalog: &Catalog, name: &str) -> Vec<String> {
    catalog
        .get_view(&QualifiedName::parse(name))
        .unwrap_or_else(|| panic!("view {:?} missing", name))
        .columns
        .clone()
}

/// Shared catalog for diagnostic-quality tests.
fn diag_catalog() -> Catalog {
    catalog(
        r#"
        CREATE TABLE users (
            id SERIAL PRIMARY KEY,
            name VARCHAR(100) NOT NULL,
            email TEXT
        );
        CREATE TABLE orders (
            id SERIAL PRIMARY KEY,
            user_id INTEGER NOT NULL,
            total NUMERIC(10, 2),
            note TEXT
        );
        "#,
    )
}

// =====================================================================
// 1. CREATE TABLE basics
// =====================================================================

#[test]
fn create_table_preserves_column_order_and_count() {
    let c = catalog("CREATE TABLE t (c3 int, a1 text, b2 boolean);");
    let t = table(&c, "t");
    assert_eq!(t.column_names(), vec!["c3", "a1", "b2"]);
    assert_eq!(t.name, QualifiedName::new("t"));
}

#[test]
fn create_table_is_registered_in_public_schema() {
    let c = catalog("CREATE TABLE t (id int);");
    assert!(c.get_table(&QualifiedName::new("t")).is_some());
    assert!(c
        .get_table(&QualifiedName::with_schema("public", "t"))
        .is_some());
    assert_eq!(
        c.table_names(),
        vec![QualifiedName::with_schema("public", "t")]
    );
}

#[test]
fn create_table_if_not_exists_creates_new_table() {
    let c = catalog("CREATE TABLE IF NOT EXISTS t (id int, name text);");
    assert_eq!(table(&c, "t").column_names(), vec!["id", "name"]);
    assert_clean(&c, "SELECT id, name FROM t");
}

#[test]
fn create_table_columns_default_to_nullable() {
    let c = catalog("CREATE TABLE t (a int, b int NULL, c int NOT NULL);");
    assert!(nullable(&c, "t", "a"));
    assert!(nullable(&c, "t", "b"));
    assert!(!nullable(&c, "t", "c"));
}

#[test]
fn create_table_unquoted_columns_are_case_insensitive_in_queries() {
    let c = catalog("CREATE TABLE users (id int, name text);");
    assert_clean(&c, "SELECT ID, Name, NAME FROM users");
    assert_clean(&c, "SELECT users.ID FROM users WHERE users.Name = 'x'");
}

#[test]
fn create_table_get_column_is_case_insensitive() {
    let c = catalog("CREATE TABLE t (MixedCase int);");
    let t = table(&c, "t");
    assert!(t.get_column("mixedcase").is_some());
    assert!(t.get_column("MIXEDCASE").is_some());
    assert!(t.column_exists("MixedCase"));
}

#[test]
fn create_table_empty_column_list() {
    // PostgreSQL allows zero-column tables.
    let c = catalog("CREATE TABLE empty_t ();");
    assert!(table(&c, "empty_t").columns.is_empty());
    assert_codes(&c, "SELECT x FROM empty_t", &["E0002"]);
}

// ---------- quoted identifiers ----------

#[test]
fn quoted_identifiers_preserve_case_in_catalog() {
    let c = catalog(r#"CREATE TABLE "User" ("createdAt" timestamp, "Id" int);"#);
    assert!(c.get_table(&QualifiedName::new("User")).is_some());
    assert!(c.get_table(&QualifiedName::new("user")).is_none());
    assert_eq!(table(&c, "User").column_names(), vec!["createdAt", "Id"]);
}

#[test]
fn quoted_identifiers_queried_with_quotes() {
    let c = catalog(r#"CREATE TABLE "Users" ("Id" int, "firstName" text);"#);
    assert_clean(&c, r#"SELECT "Id", "firstName" FROM "Users""#);
    assert_clean(
        &c,
        r#"SELECT u."firstName" FROM "Users" u WHERE u."Id" = 1"#,
    );
}

#[test]
fn quoted_identifier_unknown_column_suggests_quoted_name() {
    let c = catalog(r#"CREATE TABLE "Users" ("Id" int, "firstName" text);"#);
    let d = single(&c, r#"SELECT "lastName" FROM "Users""#);
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(d.message, "Column 'lastName' not found in table 'Users'");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'firstName'?"));
    // Span covers the quoted identifier including its quotes.
    assert_eq!(loc(&d), (1, 8, 10));
}

#[test]
fn quoted_reserved_word_table_with_alter() {
    let c = catalog(
        r#"
        CREATE TABLE "Order" (id int);
        ALTER TABLE "Order" ADD COLUMN "userId" int;
        "#,
    );
    assert_eq!(table(&c, "Order").column_names(), vec!["id", "userId"]);
    assert_clean(&c, r#"SELECT "userId" FROM "Order""#);
}

#[test]
fn quoted_table_not_found_message_has_no_quotes() {
    let c = catalog(r#"CREATE TABLE "Users" (id int);"#);
    let diags = assert_codes(&c, r#"SELECT 1 FROM "Accounts""#, &["E0001"]);
    assert_eq!(diags[0].message, "Table 'Accounts' not found");
    assert_eq!(loc(&diags[0]), (1, 15, 10));
}

// ---------- schemas ----------

#[test]
fn create_schema_and_schema_qualified_table() {
    let c = catalog(
        r#"
        CREATE SCHEMA app;
        CREATE TABLE app.accounts (id bigint, owner text);
        "#,
    );
    assert!(c
        .get_table(&QualifiedName::with_schema("app", "accounts"))
        .is_some());
    assert!(
        c.get_table(&QualifiedName::new("accounts")).is_none(),
        "app.accounts must not be visible as public.accounts"
    );
    assert!(c.schemas.contains_key("app"));
}

#[test]
fn schema_qualified_table_queries() {
    let c = catalog(
        "CREATE SCHEMA IF NOT EXISTS app; CREATE TABLE app.accounts (id bigint, owner text);",
    );
    assert_clean(&c, "SELECT id, owner FROM app.accounts");
    assert_clean(&c, "SELECT a.id FROM app.accounts a WHERE a.owner = 'x'");
    assert_clean(&c, "SELECT accounts.id FROM app.accounts");
    let d = single(&c, "SELECT nope FROM app.accounts");
    assert_eq!(d.message, "Column 'nope' not found in table 'app.accounts'");
}

#[test]
fn schema_qualified_table_not_visible_unqualified() {
    // With the default search_path (public), app.accounts is not reachable as `accounts`.
    let c = catalog("CREATE TABLE app.accounts (id bigint);");
    let diags = analyze(&c, "SELECT * FROM accounts");
    assert_eq!(diags[0].kind, DiagnosticKind::TableNotFound);
    assert_eq!(diags[0].message, "Table 'accounts' not found");
}

#[test]
fn unknown_schema_table_not_found_message_is_qualified() {
    let c = catalog("CREATE TABLE app.accounts (id bigint);");
    let diags = analyze(&c, "SELECT * FROM other.accounts");
    assert_eq!(diags[0].kind, DiagnosticKind::TableNotFound);
    assert_eq!(diags[0].message, "Table 'other.accounts' not found");
    // Span points at the last identifier (table name).
    assert_eq!(loc(&diags[0]), (1, 21, 8));
}

#[test]
fn explicit_public_schema_is_same_as_unqualified() {
    let c = catalog("CREATE TABLE public.p (id int); CREATE TABLE q (id int);");
    assert_clean(&c, "SELECT id FROM p");
    assert_clean(&c, "SELECT id FROM public.p");
    assert_clean(&c, "SELECT id FROM public.q");
}

#[test]
fn same_table_name_in_two_schemas_is_distinct() {
    let c = catalog(
        r#"
        CREATE TABLE public.items (id int, price numeric);
        CREATE TABLE archive.items (id int, archived_at timestamp);
        "#,
    );
    assert_clean(&c, "SELECT price FROM items");
    assert_clean(&c, "SELECT archived_at FROM archive.items");
    assert_codes(&c, "SELECT archived_at FROM items", &["E0002"]);
    assert_codes(&c, "SELECT price FROM archive.items", &["E0002"]);
}

// =====================================================================
// 2. PostgreSQL data types
// =====================================================================

#[test]
fn types_serial_variants() {
    let c = catalog(
        "CREATE TABLE t (a serial, b bigserial, c smallserial, d serial4, e serial8, f serial2);",
    );
    assert_eq!(col_type(&c, "t", "a"), SqlType::Integer);
    assert_eq!(col_type(&c, "t", "b"), SqlType::BigInt);
    assert_eq!(col_type(&c, "t", "c"), SqlType::SmallInt);
    assert_eq!(col_type(&c, "t", "d"), SqlType::Integer);
    assert_eq!(col_type(&c, "t", "e"), SqlType::BigInt);
    assert_eq!(col_type(&c, "t", "f"), SqlType::SmallInt);
}

#[test]
fn types_integer_aliases() {
    let c =
        catalog("CREATE TABLE t (a int, b integer, c int2, d int4, e int8, f smallint, g bigint);");
    assert_eq!(col_type(&c, "t", "a"), SqlType::Integer);
    assert_eq!(col_type(&c, "t", "b"), SqlType::Integer);
    assert_eq!(col_type(&c, "t", "c"), SqlType::SmallInt);
    assert_eq!(col_type(&c, "t", "d"), SqlType::Integer);
    assert_eq!(col_type(&c, "t", "e"), SqlType::BigInt);
    assert_eq!(col_type(&c, "t", "f"), SqlType::SmallInt);
    assert_eq!(col_type(&c, "t", "g"), SqlType::BigInt);
}

#[test]
fn types_floating_and_numeric() {
    let c = catalog(
        "CREATE TABLE t (a double precision, b float8, c real, d float4, e numeric(10,2), f decimal, g numeric(5));",
    );
    assert_eq!(col_type(&c, "t", "a"), SqlType::DoublePrecision);
    assert_eq!(col_type(&c, "t", "b"), SqlType::DoublePrecision);
    assert_eq!(col_type(&c, "t", "c"), SqlType::Real);
    assert_eq!(col_type(&c, "t", "d"), SqlType::Real);
    assert_eq!(
        col_type(&c, "t", "e"),
        SqlType::Decimal {
            precision: Some(10),
            scale: Some(2)
        }
    );
    assert_eq!(
        col_type(&c, "t", "f"),
        SqlType::Decimal {
            precision: None,
            scale: None
        }
    );
    assert_eq!(
        col_type(&c, "t", "g"),
        SqlType::Decimal {
            precision: Some(5),
            scale: None
        }
    );
}

#[test]
fn types_character() {
    let c = catalog(
        "CREATE TABLE t (a varchar, b varchar(10), c character varying(20), d char(3), e character(5), f text, g char);",
    );
    assert_eq!(col_type(&c, "t", "a"), SqlType::Varchar { length: None });
    assert_eq!(
        col_type(&c, "t", "b"),
        SqlType::Varchar { length: Some(10) }
    );
    assert_eq!(
        col_type(&c, "t", "c"),
        SqlType::Varchar { length: Some(20) }
    );
    assert_eq!(col_type(&c, "t", "d"), SqlType::Char { length: Some(3) });
    assert_eq!(col_type(&c, "t", "e"), SqlType::Char { length: Some(5) });
    assert_eq!(col_type(&c, "t", "f"), SqlType::Text);
    assert_eq!(col_type(&c, "t", "g"), SqlType::Char { length: None });
}

#[test]
fn types_varchar_with_collation() {
    let c = catalog(r#"CREATE TABLE t (a varchar(255) COLLATE "C" NOT NULL);"#);
    assert_eq!(
        col_type(&c, "t", "a"),
        SqlType::Varchar { length: Some(255) }
    );
    assert!(!nullable(&c, "t", "a"));
}

#[test]
fn types_date_time() {
    let c = catalog(
        r#"CREATE TABLE t (
            a timestamp(3) with time zone,
            b timestamp without time zone,
            c timestamp(6),
            d time with time zone,
            e time(3),
            f date,
            g interval
        );"#,
    );
    assert_eq!(
        col_type(&c, "t", "a"),
        SqlType::Timestamp {
            precision: Some(3),
            with_timezone: true
        }
    );
    assert_eq!(
        col_type(&c, "t", "b"),
        SqlType::Timestamp {
            precision: None,
            with_timezone: false
        }
    );
    assert_eq!(
        col_type(&c, "t", "c"),
        SqlType::Timestamp {
            precision: Some(6),
            with_timezone: false
        }
    );
    assert_eq!(
        col_type(&c, "t", "d"),
        SqlType::Time {
            precision: None,
            with_timezone: true
        }
    );
    assert_eq!(
        col_type(&c, "t", "e"),
        SqlType::Time {
            precision: Some(3),
            with_timezone: false
        }
    );
    assert_eq!(col_type(&c, "t", "f"), SqlType::Date);
    assert_eq!(col_type(&c, "t", "g"), SqlType::Interval);
}

#[test]
fn types_uuid_json_bool_bytea() {
    let c = catalog("CREATE TABLE t (a uuid, b json, c jsonb, d bool, e boolean, f bytea);");
    assert_eq!(col_type(&c, "t", "a"), SqlType::Uuid);
    assert_eq!(col_type(&c, "t", "b"), SqlType::Json);
    assert_eq!(col_type(&c, "t", "c"), SqlType::Jsonb);
    assert_eq!(col_type(&c, "t", "d"), SqlType::Boolean);
    assert_eq!(col_type(&c, "t", "e"), SqlType::Boolean);
    assert_eq!(col_type(&c, "t", "f"), SqlType::Bytea);
}

#[test]
fn types_extension_and_special_types_are_custom() {
    let c = catalog(
        "CREATE TABLE t (a citext, b inet, c money, d tsvector, e int4range, f xml, g cidr, h macaddr, i hstore, j tstzrange);",
    );
    for (col, name) in [
        ("a", "citext"),
        ("b", "inet"),
        ("c", "money"),
        ("d", "tsvector"),
        ("e", "int4range"),
        ("f", "xml"),
        ("g", "cidr"),
        ("h", "macaddr"),
        ("i", "hstore"),
        ("j", "tstzrange"),
    ] {
        assert_eq!(
            col_type(&c, "t", col),
            SqlType::Custom(name.to_string()),
            "column {}",
            col
        );
    }
    // Queries selecting these columns resolve fine.
    assert_clean(&c, "SELECT a, b, c, d, e, f, g, h, i, j FROM t");
}

#[test]
fn types_schema_qualified_custom_type() {
    let c = catalog("CREATE TABLE t (a public.citext);");
    assert_eq!(
        col_type(&c, "t", "a"),
        SqlType::Custom("public.citext".to_string())
    );
}

#[test]
fn types_arrays() {
    let c = catalog(
        "CREATE TABLE t (a int[], b text[][], c int[3], d varchar(10)[], e uuid[], f jsonb[]);",
    );
    assert_eq!(
        col_type(&c, "t", "a"),
        SqlType::Array(Box::new(SqlType::Integer))
    );
    assert_eq!(
        col_type(&c, "t", "b"),
        SqlType::Array(Box::new(SqlType::Array(Box::new(SqlType::Text))))
    );
    assert_eq!(
        col_type(&c, "t", "c"),
        SqlType::Array(Box::new(SqlType::Integer))
    );
    assert_eq!(
        col_type(&c, "t", "d"),
        SqlType::Array(Box::new(SqlType::Varchar { length: Some(10) }))
    );
    assert_eq!(
        col_type(&c, "t", "e"),
        SqlType::Array(Box::new(SqlType::Uuid))
    );
    assert_eq!(
        col_type(&c, "t", "f"),
        SqlType::Array(Box::new(SqlType::Jsonb))
    );
    assert_eq!(col_type(&c, "t", "b").display_name(), "text[][]");
}

#[test]
fn types_array_with_defaults() {
    let c = catalog(
        "CREATE TABLE t (a text[] DEFAULT '{}', b integer[] NOT NULL DEFAULT ARRAY[]::integer[]);",
    );
    assert!(nullable(&c, "t", "a"));
    assert!(!nullable(&c, "t", "b"));
    assert!(matches!(
        table(&c, "t").get_column("b").unwrap().default,
        Some(DefaultValue::Expression(_))
    ));
}

#[test]
fn types_display_names() {
    let c = catalog(
        "CREATE TABLE t (a numeric(10,2), b varchar(5), c timestamp(3) with time zone, d int[]);",
    );
    assert_eq!(col_type(&c, "t", "a").display_name(), "numeric(10,2)");
    assert_eq!(col_type(&c, "t", "b").display_name(), "varchar(5)");
    assert_eq!(
        col_type(&c, "t", "c").display_name(),
        "timestamp with time zone"
    );
    assert_eq!(col_type(&c, "t", "d").display_name(), "integer[]");
}

#[test]
fn types_domain_and_composite_columns_are_custom() {
    let c = catalog(
        r#"
        CREATE DOMAIN posint AS integer CHECK (VALUE > 0);
        CREATE TYPE address AS (street text, city text);
        CREATE TABLE t (id posint, addr address);
        "#,
    );
    assert_eq!(col_type(&c, "t", "id"), SqlType::Custom("posint".into()));
    assert_eq!(col_type(&c, "t", "addr"), SqlType::Custom("address".into()));
    assert_clean(&c, "SELECT id, addr FROM t");
    // Composite types are not registered as enums.
    assert!(!c.enum_exists("address"));
}

// ---------- identity / generated ----------

#[test]
fn identity_always_and_by_default() {
    let c = catalog(
        r#"CREATE TABLE t (
            a int GENERATED ALWAYS AS IDENTITY,
            b bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
            c int
        );"#,
    );
    let t = table(&c, "t");
    assert!(matches!(
        t.get_column("a").unwrap().identity,
        Some(IdentityKind::Always)
    ));
    assert!(matches!(
        t.get_column("b").unwrap().identity,
        Some(IdentityKind::ByDefault)
    ));
    assert!(t.get_column("c").unwrap().identity.is_none());
    assert!(
        !t.get_column("a").unwrap().nullable,
        "identity implies NOT NULL"
    );
    assert!(!t.get_column("b").unwrap().nullable);
}

#[test]
fn identity_added_via_alter_table() {
    let c = catalog(
        r#"
        CREATE TABLE t (name text);
        ALTER TABLE t ADD COLUMN id bigint GENERATED BY DEFAULT AS IDENTITY;
        "#,
    );
    let col = table(&c, "t").get_column("id").unwrap();
    assert!(matches!(col.identity, Some(IdentityKind::ByDefault)));
    assert!(!col.nullable);
    assert_eq!(col.data_type, SqlType::BigInt);
}

#[test]
fn generated_stored_column_is_not_identity() {
    let c = catalog(
        r#"CREATE TABLE t (
            price numeric(10,2),
            qty int,
            total numeric GENERATED ALWAYS AS (price * qty) STORED
        );"#,
    );
    let col = table(&c, "t").get_column("total").unwrap();
    assert!(col.identity.is_none(), "computed column is not IDENTITY");
    assert!(col.nullable);
    assert_clean(&c, "SELECT total FROM t WHERE total > 10");
}

// ---------- defaults ----------

#[test]
fn default_value_classification() {
    let c = catalog(
        r#"CREATE TABLE t (
            a timestamptz DEFAULT now(),
            b timestamp DEFAULT CURRENT_TIMESTAMP,
            c int DEFAULT nextval('t_c_seq'::regclass),
            d text DEFAULT 'x',
            e text DEFAULT NULL,
            f uuid DEFAULT gen_random_uuid(),
            g boolean DEFAULT false,
            h int DEFAULT -1,
            i jsonb DEFAULT '{}'::jsonb,
            j date DEFAULT CURRENT_DATE,
            k int
        );"#,
    );
    let t = table(&c, "t");
    let d = |n: &str| t.get_column(n).unwrap().default.clone();
    assert!(matches!(d("a"), Some(DefaultValue::CurrentTimestamp)));
    assert!(matches!(d("b"), Some(DefaultValue::CurrentTimestamp)));
    assert!(matches!(d("c"), Some(DefaultValue::NextVal(ref s)) if s.contains("t_c_seq")));
    assert!(matches!(d("d"), Some(DefaultValue::Literal(ref s)) if s == "'x'"));
    assert!(matches!(d("e"), Some(DefaultValue::Null)));
    assert!(matches!(d("f"), Some(DefaultValue::Expression(ref s)) if s == "gen_random_uuid()"));
    assert!(matches!(d("g"), Some(DefaultValue::Literal(ref s)) if s == "false"));
    assert!(matches!(d("h"), Some(DefaultValue::Expression(ref s)) if s == "-1"));
    assert!(matches!(d("i"), Some(DefaultValue::Expression(_))));
    assert!(matches!(d("j"), Some(DefaultValue::Expression(ref s)) if s == "CURRENT_DATE"));
    assert!(d("k").is_none());
}

#[test]
fn default_with_semicolon_in_string_literal() {
    let c = catalog("CREATE TABLE t (s text DEFAULT 'a; b', n int); CREATE TABLE u (id int);");
    assert!(has_table(&c, "t"));
    assert!(has_table(&c, "u"));
    assert!(matches!(
        table(&c, "t").get_column("s").unwrap().default,
        Some(DefaultValue::Literal(ref s)) if s == "'a; b'"
    ));
}

// ---------- constraints ----------

#[test]
fn check_constraints_column_and_table_level() {
    let c = catalog(
        r#"CREATE TABLE t (
            id int CONSTRAINT positive_id CHECK (id > 0),
            status text CHECK (status IN ('a', 'b')),
            lo int, hi int,
            CONSTRAINT lo_le_hi CHECK (lo <= hi)
        );"#,
    );
    let t = table(&c, "t");
    assert_eq!(t.check_constraints.len(), 3);
    let names: Vec<Option<&str>> = t
        .check_constraints
        .iter()
        .map(|c| c.name.as_deref())
        .collect();
    assert_eq!(names, vec![Some("positive_id"), None, Some("lo_le_hi")]);
    assert_eq!(t.check_constraints[2].expression, "lo <= hi");
    assert_clean(&c, "SELECT id, status, lo, hi FROM t");
}

#[test]
fn table_level_composite_primary_key() {
    let c = catalog("CREATE TABLE t (a int, b int, c text, CONSTRAINT t_pkey PRIMARY KEY (a, b));");
    let t = table(&c, "t");
    let pk = t.primary_key.as_ref().expect("primary key recorded");
    assert_eq!(pk.name.as_deref(), Some("t_pkey"));
    assert_eq!(pk.columns, vec!["a", "b"]);
    assert!(t.get_column("a").unwrap().is_primary_key);
    assert!(!t.get_column("a").unwrap().nullable);
    assert!(!t.get_column("b").unwrap().nullable);
    assert!(t.get_column("c").unwrap().nullable);
}

#[test]
fn table_level_unique_and_foreign_keys() {
    let c = catalog(
        r#"
        CREATE TABLE parent (id int PRIMARY KEY, code text);
        CREATE TABLE child (
            id int,
            parent_id int,
            code text,
            UNIQUE (parent_id, code),
            CONSTRAINT child_parent_fk FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE CASCADE,
            FOREIGN KEY (code) REFERENCES app.codes (code)
        );
        "#,
    );
    let t = table(&c, "child");
    assert_eq!(t.unique_constraints.len(), 1);
    assert_eq!(t.unique_constraints[0].columns, vec!["parent_id", "code"]);
    assert_eq!(t.foreign_keys.len(), 2);
    let fk = &t.foreign_keys[0];
    assert_eq!(fk.name.as_deref(), Some("child_parent_fk"));
    assert_eq!(fk.columns, vec!["parent_id"]);
    assert_eq!(fk.references_table, QualifiedName::new("parent"));
    assert_eq!(fk.references_columns, vec!["id"]);
    assert_eq!(
        t.foreign_keys[1].references_table,
        QualifiedName::with_schema("app", "codes")
    );
}

#[test]
fn inline_primary_key_marks_column() {
    let c = catalog("CREATE TABLE t (id int PRIMARY KEY, n text);");
    let col = table(&c, "t").get_column("id").unwrap();
    assert!(col.is_primary_key);
    assert!(!col.nullable);
}

// ---------- partitioning / temp / comments ----------

#[test]
fn partitioned_parent_tables() {
    let c = catalog(
        r#"
        CREATE TABLE m1 (id int, created date) PARTITION BY RANGE (created);
        CREATE TABLE m2 (id int, k text) PARTITION BY LIST (k);
        CREATE TABLE m3 (id int, k text) PARTITION BY HASH (id);
        "#,
    );
    assert_eq!(table(&c, "m1").column_names(), vec!["id", "created"]);
    assert_clean(&c, "SELECT k FROM m2");
    assert_clean(&c, "SELECT k FROM m3 WHERE id = 1");
}

#[test]
fn temp_tables() {
    let c = catalog("CREATE TEMP TABLE t1 (id int); CREATE TEMPORARY TABLE t2 (id int, n text);");
    assert_clean(&c, "SELECT id FROM t1");
    assert_clean(&c, "SELECT n FROM t2");
}

#[test]
fn comment_on_is_ignored() {
    let c = catalog(
        r#"
        CREATE TABLE t (id int);
        COMMENT ON TABLE t IS 'has; semicolon';
        COMMENT ON COLUMN t.id IS 'identifier';
        CREATE TABLE u (id int);
        "#,
    );
    assert!(has_table(&c, "t"));
    assert!(has_table(&c, "u"));
}

// =====================================================================
// 3. Migration sequences (ALTER / DROP / multiple files)
// =====================================================================

#[test]
fn alter_add_column_if_not_exists() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE u ADD COLUMN IF NOT EXISTS b text;");
    assert_eq!(table(&c, "u").column_names(), vec!["id", "b"]);
    assert_clean(&c, "SELECT b FROM u");
}

#[test]
fn alter_add_without_column_keyword() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE u ADD x int;");
    assert_clean(&c, "SELECT x FROM u");
}

#[test]
fn alter_add_column_not_null_default() {
    let c = catalog(
        "CREATE TABLE u (id int); ALTER TABLE u ADD COLUMN status text NOT NULL DEFAULT 'new';",
    );
    let col = table(&c, "u").get_column("status").unwrap();
    assert!(!col.nullable);
    assert!(matches!(col.default, Some(DefaultValue::Literal(ref s)) if s == "'new'"));
}

#[test]
fn alter_add_column_with_check_records_constraint() {
    let c = catalog(
        "CREATE TABLE u (id int); ALTER TABLE u ADD COLUMN age int CONSTRAINT age_ok CHECK (age >= 0);",
    );
    let t = table(&c, "u");
    assert_eq!(t.check_constraints.len(), 1);
    assert_eq!(t.check_constraints[0].name.as_deref(), Some("age_ok"));
}

#[test]
fn alter_multiple_operations_in_one_statement() {
    let c = catalog(
        r#"
        CREATE TABLE u (id int, a int);
        ALTER TABLE u
            ADD COLUMN b int REFERENCES u(id),
            ADD COLUMN c text NOT NULL DEFAULT 'x',
            DROP COLUMN a;
        "#,
    );
    assert_eq!(table(&c, "u").column_names(), vec!["id", "b", "c"]);
    assert_clean(&c, "SELECT b, c FROM u");
    let d = single(&c, "SELECT a FROM u");
    assert_eq!(d.message, "Column 'a' not found in table 'u'");
}

#[test]
fn alter_drop_column_if_exists() {
    let c = catalog(
        r#"
        CREATE TABLE u (id int, a int);
        ALTER TABLE u DROP COLUMN IF EXISTS a;
        ALTER TABLE u DROP COLUMN IF EXISTS never_existed;
        "#,
    );
    assert_eq!(table(&c, "u").column_names(), vec!["id"]);
    assert_codes(&c, "SELECT a FROM u", &["E0002"]);
}

#[test]
fn alter_drop_column_cascade() {
    let c = catalog("CREATE TABLE u (id int, a int); ALTER TABLE u DROP COLUMN a CASCADE;");
    assert_eq!(table(&c, "u").column_names(), vec!["id"]);
}

#[test]
fn alter_re_add_column_replaces_type() {
    let c = catalog(
        r#"
        CREATE TABLE u (id int);
        ALTER TABLE u ADD COLUMN x int;
        ALTER TABLE u DROP COLUMN x;
        ALTER TABLE u ADD COLUMN x text;
        "#,
    );
    assert_eq!(col_type(&c, "u", "x"), SqlType::Text);
    assert_clean(&c, "UPDATE u SET x = 'a'");
}

#[test]
fn alter_rename_column_old_name_is_error_with_suggestion() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE u RENAME COLUMN id TO uid;");
    assert_clean(&c, "SELECT uid FROM u");
    let d = single(&c, "SELECT id FROM u");
    assert_eq!(d.kind, DiagnosticKind::ColumnNotFound);
    assert_eq!(
        d.help.as_deref(),
        Some("'id' was renamed to 'uid' by ALTER TABLE in the schema")
    );
}

#[test]
fn alter_rename_column_keeps_type_and_nullability() {
    let c =
        catalog("CREATE TABLE u (email varchar(50) NOT NULL); ALTER TABLE u RENAME email TO mail;");
    let col = table(&c, "u").get_column("mail").unwrap();
    assert_eq!(col.name, "mail");
    assert_eq!(col.data_type, SqlType::Varchar { length: Some(50) });
    assert!(!col.nullable);
}

#[test]
fn alter_rename_table_old_and_new_name() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE u RENAME TO v;");
    assert!(!has_table(&c, "u"));
    assert_eq!(table(&c, "v").name, QualifiedName::new("v"));
    assert_clean(&c, "SELECT id FROM v");
    let diags = analyze(&c, "SELECT id FROM u");
    assert_eq!(diags[0].kind, DiagnosticKind::TableNotFound);
    assert_eq!(diags[0].message, "Table 'u' not found");
}

#[test]
fn alter_table_if_exists_rename() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE IF EXISTS u RENAME TO w;");
    assert!(has_table(&c, "w"));
    assert!(!has_table(&c, "u"));
}

#[test]
fn alter_rename_to_schema_qualified_name() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE u RENAME TO public.w;");
    assert_clean(&c, "SELECT id FROM w");
}

#[test]
fn alter_rename_then_alter_new_name() {
    let c = catalog(
        r#"
        CREATE TABLE u (id int);
        ALTER TABLE u RENAME TO people;
        ALTER TABLE people ADD COLUMN full_name text;
        "#,
    );
    assert_eq!(table(&c, "people").column_names(), vec!["id", "full_name"]);
}

#[test]
fn alter_schema_qualified_table() {
    let c = catalog(
        r#"
        CREATE TABLE app.u (id int);
        ALTER TABLE app.u ADD COLUMN n text;
        ALTER TABLE app.u RENAME TO w;
        "#,
    );
    assert_clean(&c, "SELECT n FROM app.w");
    assert_eq!(
        analyze(&c, "SELECT n FROM app.u")[0].kind,
        DiagnosticKind::TableNotFound
    );
}

#[test]
fn alter_public_qualified_matches_unqualified_table() {
    let c = catalog("CREATE TABLE u (id int); ALTER TABLE public.u ADD COLUMN x int;");
    assert_clean(&c, "SELECT x FROM u");
    let c = catalog("CREATE TABLE public.u (id int); ALTER TABLE u ADD COLUMN x int;");
    assert_clean(&c, "SELECT x FROM u");
}

#[test]
fn alter_add_constraints() {
    let c = catalog(
        r#"
        CREATE TABLE p (id int);
        CREATE TABLE u (id int, p_id int, email text, age int);
        ALTER TABLE u ADD CONSTRAINT u_pkey PRIMARY KEY (id);
        ALTER TABLE ONLY u ADD CONSTRAINT u_email_key UNIQUE (email);
        ALTER TABLE ONLY u ADD CONSTRAINT u_p_fk FOREIGN KEY (p_id) REFERENCES p(id);
        ALTER TABLE u ADD CONSTRAINT u_age_ck CHECK (age > 0);
        "#,
    );
    let t = table(&c, "u");
    let pk = t.primary_key.as_ref().unwrap();
    assert_eq!(pk.name.as_deref(), Some("u_pkey"));
    assert!(!t.get_column("id").unwrap().nullable);
    assert!(t.get_column("id").unwrap().is_primary_key);
    assert_eq!(t.unique_constraints[0].name.as_deref(), Some("u_email_key"));
    assert_eq!(t.foreign_keys[0].references_table, QualifiedName::new("p"));
    assert_eq!(t.check_constraints[0].name.as_deref(), Some("u_age_ck"));
}

#[test]
fn alter_unknown_table_warns_but_parse_succeeds() {
    let (c, warnings) =
        catalog_with_warnings("CREATE TABLE u (id int); ALTER TABLE nope ADD COLUMN x int;");
    assert!(has_table(&c, "u"));
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.severity, Severity::Warning);
    assert_eq!(w.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        w.message,
        "ALTER TABLE references table 'nope' which was not found in schema"
    );
    assert_eq!(
        w.help.as_deref(),
        Some("Ensure the CREATE TABLE statement appears before ALTER TABLE")
    );
}

#[test]
fn alter_non_schema_operations_on_unknown_table_do_not_warn() {
    let (_, warnings) = catalog_with_warnings(
        "ALTER TABLE nope OWNER TO bob; ALTER TABLE nope ENABLE ROW LEVEL SECURITY;",
    );
    assert!(warnings.is_empty(), "{:?}", warnings);
}

#[test]
fn alter_after_drop_warns() {
    let (_, warnings) = catalog_with_warnings(
        "CREATE TABLE u (id int); DROP TABLE u; ALTER TABLE u ADD COLUMN x int;",
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("'u'"));
}

#[test]
fn drop_table_removes_table() {
    let c = catalog("CREATE TABLE u (id int); CREATE TABLE w (id int); DROP TABLE u;");
    assert!(!has_table(&c, "u"));
    assert!(has_table(&c, "w"));
    assert_eq!(
        analyze(&c, "SELECT * FROM u")[0].kind,
        DiagnosticKind::TableNotFound
    );
}

#[test]
fn drop_table_if_exists_unknown_is_silent() {
    let c = catalog(
        "CREATE TABLE u (id int); DROP TABLE IF EXISTS nope CASCADE; DROP TABLE IF EXISTS app.nope;",
    );
    assert!(has_table(&c, "u"));
}

#[test]
fn drop_table_multiple_names() {
    let c = catalog(
        "CREATE TABLE a (id int); CREATE TABLE b (id int); CREATE TABLE c (id int); DROP TABLE a, b;",
    );
    assert!(!has_table(&c, "a"));
    assert!(!has_table(&c, "b"));
    assert!(has_table(&c, "c"));
}

#[test]
fn drop_table_public_qualified() {
    let c = catalog("CREATE TABLE u (id int); DROP TABLE public.u;");
    assert!(!has_table(&c, "u"));
}

#[test]
fn create_table_after_drop_uses_new_definition() {
    let c = catalog("CREATE TABLE u (id int); DROP TABLE u; CREATE TABLE u (other int);");
    assert_eq!(table(&c, "u").column_names(), vec!["other"]);
    assert_clean(&c, "SELECT other FROM u");
    assert_codes(&c, "SELECT id FROM u", &["E0002"]);
}

#[test]
fn multiple_files_applied_in_order() {
    let (c, warnings) = catalog_from_files(&[
        "CREATE TABLE a (id int);",
        "ALTER TABLE a ADD COLUMN n text; CREATE TABLE b (id int);",
        "ALTER TABLE a RENAME COLUMN n TO title; DROP TABLE b;",
        // A file that needs the resilient fallback path still applies its statements.
        "CREATE FUNCTION f() RETURNS int AS $$ SELECT 1; $$ LANGUAGE sql; ALTER TABLE a ADD COLUMN z int;",
    ]);
    assert!(warnings.is_empty(), "{:?}", warnings);
    assert_eq!(table(&c, "a").column_names(), vec!["id", "title", "z"]);
    assert!(!has_table(&c, "b"));
}

#[test]
fn multiple_files_alter_before_create_warns() {
    let (c, warnings) = catalog_from_files(&[
        "ALTER TABLE a ADD COLUMN n text;",
        "CREATE TABLE a (id int);",
    ]);
    assert_eq!(warnings.len(), 1, "ALTER in earlier file should warn");
    assert_eq!(table(&c, "a").column_names(), vec!["id"]);
}

#[test]
fn prisma_style_migration_sequence() {
    let (c, warnings) = catalog_from_files(&[
        r#"
        -- CreateTable
        CREATE TABLE "User" (
            "id" SERIAL NOT NULL,
            "email" TEXT NOT NULL,
            "name" TEXT,
            CONSTRAINT "User_pkey" PRIMARY KEY ("id")
        );
        -- CreateIndex
        CREATE UNIQUE INDEX "User_email_key" ON "User"("email");
        "#,
        r#"
        -- AlterTable
        ALTER TABLE "User" ADD COLUMN "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP;
        -- CreateTable
        CREATE TABLE "Post" (
            "id" SERIAL NOT NULL,
            "authorId" INTEGER NOT NULL,
            CONSTRAINT "Post_pkey" PRIMARY KEY ("id")
        );
        -- AddForeignKey
        ALTER TABLE "Post" ADD CONSTRAINT "Post_authorId_fkey" FOREIGN KEY ("authorId") REFERENCES "User"("id") ON DELETE RESTRICT ON UPDATE CASCADE;
        "#,
    ]);
    assert!(warnings.is_empty(), "{:?}", warnings);
    assert_eq!(
        table(&c, "User").column_names(),
        vec!["id", "email", "name", "createdAt"]
    );
    assert_eq!(table(&c, "Post").foreign_keys.len(), 1);
    assert_clean(
        &c,
        r#"SELECT u."email", p."id" FROM "User" u JOIN "Post" p ON p."authorId" = u."id" WHERE u."name" = 'x'"#,
    );
}

// =====================================================================
// 4. Views
// =====================================================================

const VIEW_BASE: &str = r#"
    CREATE TABLE u (id int, n text);
    CREATE TABLE o (id int, uid int, amount numeric);
"#;

fn view_catalog(extra: &str) -> Catalog {
    catalog(&format!("{}\n{}", VIEW_BASE, extra))
}

#[test]
fn view_column_inference_with_aliases_and_expressions() {
    let c = view_catalog("CREATE VIEW v AS SELECT id AS user_id, n || 'x' AS nx, u.n FROM u;");
    assert_eq!(view_columns(&c, "v"), vec!["user_id", "nx", "n"]);
    assert_clean(&c, "SELECT user_id, nx, n FROM v");
    let d = single(&c, "SELECT id FROM v");
    assert_eq!(d.message, "Column 'id' not found in view 'v'");
}

#[test]
fn view_literal_columns_with_aliases() {
    let c = catalog("CREATE VIEW consts AS SELECT 1 AS one, 'x'::text AS two;");
    assert_eq!(view_columns(&c, "consts"), vec!["one", "two"]);
    assert_clean(&c, "SELECT one, two FROM consts");
}

#[test]
fn view_select_star_expands_table_columns() {
    let c = view_catalog("CREATE VIEW v AS SELECT * FROM u;");
    assert_eq!(view_columns(&c, "v"), vec!["id", "n"]);
    assert_clean(&c, "SELECT id, n FROM v");
}

#[test]
fn view_select_star_over_comma_join() {
    let c = view_catalog("CREATE VIEW v AS SELECT * FROM u, o;");
    assert_eq!(
        view_columns(&c, "v"),
        vec!["id", "n", "id", "uid", "amount"]
    );
    assert_clean(&c, "SELECT n, amount FROM v");
}

#[test]
fn view_qualified_star_with_join() {
    let c = view_catalog("CREATE VIEW v AS SELECT u.*, o.amount FROM u JOIN o ON o.uid = u.id;");
    assert_eq!(view_columns(&c, "v"), vec!["id", "n", "amount"]);
    assert_clean(&c, "SELECT id, n, amount FROM v");
}

#[test]
fn view_on_view() {
    let c = view_catalog(
        r#"
        CREATE VIEW v1 AS SELECT id, n AS name FROM u;
        CREATE VIEW v2 AS SELECT * FROM v1;
        CREATE VIEW v3 AS SELECT name FROM v2 WHERE id > 0;
        "#,
    );
    assert_eq!(view_columns(&c, "v2"), vec!["id", "name"]);
    assert_eq!(view_columns(&c, "v3"), vec!["name"]);
    assert_clean(&c, "SELECT name FROM v3");
    assert_codes(&c, "SELECT id FROM v3", &["E0002"]);
}

#[test]
fn view_explicit_column_list() {
    let c = view_catalog("CREATE VIEW v (a, b) AS SELECT id, n FROM u;");
    assert_eq!(view_columns(&c, "v"), vec!["a", "b"]);
    assert_clean(&c, "SELECT a, b FROM v");
    assert_codes(&c, "SELECT id FROM v", &["E0002"]);
}

#[test]
fn view_with_cte_body() {
    let c = view_catalog("CREATE VIEW v AS WITH c AS (SELECT id FROM u) SELECT id FROM c;");
    assert_eq!(view_columns(&c, "v"), vec!["id"]);
}

#[test]
fn view_distinct_order_by() {
    let c = view_catalog("CREATE VIEW v AS SELECT DISTINCT n FROM u WHERE id > 0 ORDER BY n;");
    assert_eq!(view_columns(&c, "v"), vec!["n"]);
}

#[test]
fn materialized_view() {
    let c = view_catalog("CREATE MATERIALIZED VIEW mv AS SELECT id, n FROM u;");
    let v = c.get_view(&QualifiedName::new("mv")).unwrap();
    assert!(v.materialized);
    assert_clean(&c, "SELECT id, n FROM mv");
    assert_codes(&c, "SELECT amount FROM mv", &["E0002"]);
}

#[test]
fn regular_view_is_not_materialized() {
    let c = view_catalog("CREATE VIEW v AS SELECT id FROM u;");
    assert!(!c.get_view(&QualifiedName::new("v")).unwrap().materialized);
    assert!(c.view_exists(&QualifiedName::new("v")));
    assert!(
        !c.table_exists(&QualifiedName::new("v")),
        "views are not tables"
    );
}

#[test]
fn create_or_replace_view_updates_columns() {
    let c = view_catalog(
        r#"
        CREATE VIEW v AS SELECT id FROM u;
        CREATE OR REPLACE VIEW v AS SELECT id, n FROM u;
        "#,
    );
    assert_eq!(view_columns(&c, "v"), vec!["id", "n"]);
    assert_clean(&c, "SELECT n FROM v");
}

#[test]
fn schema_qualified_view() {
    let c = view_catalog("CREATE VIEW reporting.v AS SELECT id FROM u;");
    assert!(c
        .get_view(&QualifiedName::with_schema("reporting", "v"))
        .is_some());
    assert_clean(&c, "SELECT id FROM reporting.v");
    assert_eq!(
        analyze(&c, "SELECT id FROM v")[0].kind,
        DiagnosticKind::TableNotFound
    );
}

#[test]
fn view_qualified_unknown_column_message() {
    let c = view_catalog("CREATE VIEW v AS SELECT id FROM u;");
    let d = single(&c, "SELECT v.id, v.zz FROM v");
    assert_eq!(d.message, "Column 'zz' not found in view 'v'");
    assert_eq!(loc(&d), (1, 16, 2));
}

#[test]
fn view_joined_with_table() {
    let c = view_catalog("CREATE VIEW active AS SELECT id AS uid, n FROM u;");
    assert_clean(
        &c,
        "SELECT a.n, o.amount FROM active a JOIN o ON o.uid = a.uid",
    );
    let d = single(&c, "SELECT a.amount FROM active a JOIN o ON o.uid = a.uid");
    assert_eq!(d.message, "Column 'amount' not found in view 'active'");
}

#[test]
fn views_with_same_column_are_ambiguous() {
    let c = view_catalog(
        r#"
        CREATE VIEW v5 AS SELECT id FROM u;
        CREATE VIEW v6 AS SELECT id FROM v5;
        "#,
    );
    assert_clean(&c, "SELECT v5.id FROM v5 JOIN v6 ON v5.id = v6.id");
    let d = single(&c, "SELECT id FROM v5 JOIN v6 ON v5.id = v6.id");
    assert_eq!(d.kind, DiagnosticKind::AmbiguousColumn);
}

#[test]
fn update_and_delete_on_view_are_accepted() {
    let c = view_catalog("CREATE VIEW v AS SELECT id FROM u;");
    assert_clean(&c, "UPDATE v SET id = 1");
    assert_clean(&c, "DELETE FROM v WHERE id = 1");
}

// =====================================================================
// 5. Resilience: pg_dump output and unsupported statements
// =====================================================================

#[test]
fn empty_input_produces_empty_catalog() {
    let c = catalog("");
    assert!(c.table_names().is_empty());
    assert!(c.enums.is_empty());
    assert_eq!(c.default_schema, "public");
}

#[test]
fn whitespace_and_comment_only_input() {
    let c = catalog("  \n\t\n-- only a comment\n/* block\n comment; with semicolon */\n");
    assert!(c.table_names().is_empty());
}

#[test]
fn comments_inside_create_table() {
    let c = catalog(
        r#"
        -- leading comment; with semicolon
        CREATE TABLE u (
            id int, -- trailing comment; with semicolon
            /* block; comment */ n text
        ); /* trailing */
        "#,
    );
    assert_eq!(table(&c, "u").column_names(), vec!["id", "n"]);
}

#[test]
fn pg_dump_schema_only_preamble_and_objects() {
    let schema = r#"
--
-- PostgreSQL database dump
--

SET statement_timeout = 0;
SET lock_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

CREATE EXTENSION IF NOT EXISTS citext WITH SCHEMA public;
COMMENT ON EXTENSION citext IS 'data type for case-insensitive character strings';

SET default_tablespace = '';
SET default_table_access_method = heap;

CREATE TABLE public.accounts (
    id integer NOT NULL,
    email public.citext NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE public.accounts OWNER TO app;

CREATE SEQUENCE public.accounts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE public.accounts_id_seq OWNED BY public.accounts.id;

ALTER TABLE ONLY public.accounts ALTER COLUMN id SET DEFAULT nextval('public.accounts_id_seq'::regclass);

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);

CREATE TABLE public.sessions (
    id bigint NOT NULL,
    account_id integer NOT NULL
);

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_account_fk FOREIGN KEY (account_id) REFERENCES public.accounts(id);

CREATE INDEX sessions_account_idx ON public.sessions USING btree (account_id);

GRANT ALL ON TABLE public.accounts TO app;
REVOKE ALL ON SCHEMA public FROM PUBLIC;

--
-- PostgreSQL database dump complete
--
"#;
    let c = catalog(schema);
    let accounts = table(&c, "accounts");
    assert_eq!(accounts.column_names(), vec!["id", "email", "created_at"]);
    assert_eq!(
        accounts.primary_key.as_ref().unwrap().name.as_deref(),
        Some("accounts_pkey")
    );
    assert_eq!(table(&c, "sessions").foreign_keys.len(), 1);
    assert_clean(
        &c,
        "SELECT a.email, s.id FROM accounts a JOIN sessions s ON s.account_id = a.id",
    );
}

#[test]
fn create_function_with_dollar_body_and_semicolons() {
    let c = catalog(
        r#"
CREATE TABLE before_fn (id int);
CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at := now(); -- semicolon; inside a comment
  RAISE NOTICE 'a;b';
  RETURN NEW;
END;
$$;
CREATE TABLE after_fn (id int);
"#,
    );
    assert!(has_table(&c, "before_fn"));
    assert!(has_table(&c, "after_fn"));
}

#[test]
fn create_function_with_tagged_dollar_quote() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
CREATE OR REPLACE FUNCTION inc(x int) RETURNS int AS $fn$ SELECT x + 1; $fn$ LANGUAGE sql IMMUTABLE;
CREATE TABLE b (id int);
"#,
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn create_function_with_single_quoted_body() {
    let c = catalog(
        "CREATE TABLE a (id int);\nCREATE FUNCTION g() RETURNS text AS 'select ''a;b''' LANGUAGE sql;\nCREATE TABLE b (id int);",
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn create_trigger_is_skipped() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
CREATE TRIGGER trg BEFORE UPDATE ON a FOR EACH ROW EXECUTE FUNCTION touch();
CREATE TRIGGER trg2 AFTER INSERT OR DELETE ON a FOR EACH STATEMENT EXECUTE PROCEDURE audit();
CREATE TABLE b (id int);
"#,
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn do_blocks_are_skipped() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
DO $$ BEGIN RAISE NOTICE 'x;y'; END $$;
DO $body$ BEGIN PERFORM 1; END; $body$ LANGUAGE plpgsql;
CREATE TABLE b (id int);
"#,
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn create_index_variants_are_skipped() {
    let c = catalog(
        r#"
CREATE TABLE a (id int, email text);
CREATE INDEX CONCURRENTLY IF NOT EXISTS a_id_idx ON a (id);
CREATE UNIQUE INDEX a_email_lower ON a USING btree (lower(email)) WHERE id > 0;
CREATE INDEX a_gin ON a USING gin (to_tsvector('english', email));
CREATE TABLE b (id int);
"#,
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn grant_revoke_policy_and_rls_are_skipped() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
GRANT SELECT, INSERT ON a TO reader;
REVOKE ALL ON a FROM PUBLIC;
CREATE POLICY p ON a USING (true);
ALTER TABLE a ENABLE ROW LEVEL SECURITY;
ALTER TABLE a OWNER TO bob;
CREATE TABLE b (id int);
"#,
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn create_extension_and_domain_are_skipped() {
    let c = catalog(
        r#"
CREATE EXTENSION IF NOT EXISTS "uuid-ossp" WITH SCHEMA public;
CREATE DOMAIN email_t AS text CHECK (VALUE ~ '^[^;]+@[^;]+$');
CREATE TABLE w (id uuid DEFAULT uuid_generate_v4(), email email_t);
"#,
    );
    assert_eq!(
        col_type(&c, "w", "email"),
        SqlType::Custom("email_t".into())
    );
}

#[test]
fn unparseable_garbage_statement_is_skipped() {
    let c = catalog(
        "CREATE TABLE a (s text DEFAULT 'it''s; fine'); this is not sql at all; CREATE TABLE b (id int);",
    );
    assert!(has_table(&c, "a"));
    assert!(has_table(&c, "b"));
}

#[test]
fn unsupported_statement_between_alters_still_applies_both() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
ALTER TABLE a ADD COLUMN x int;
CREATE PROCEDURE p() LANGUAGE plpgsql AS $$ BEGIN NULL; END $$;
ALTER TABLE a ADD COLUMN y int;
"#,
    );
    assert_eq!(table(&c, "a").column_names(), vec!["id", "x", "y"]);
}

#[test]
fn enum_types_in_pg_dump_style() {
    let c = catalog(
        r#"
CREATE TYPE public.mood AS ENUM (
    'happy',
    'sad'
);
ALTER TYPE public.mood OWNER TO app;
CREATE TABLE public.people (id int, current_mood public.mood);
"#,
    );
    // Enums are keyed by unqualified name.
    assert_eq!(c.get_enum("mood").unwrap().values, vec!["happy", "sad"]);
    assert_eq!(
        col_type(&c, "people", "current_mood"),
        SqlType::Custom("public.mood".into())
    );
}

#[test]
fn enum_with_quoted_values_and_escapes() {
    let c = catalog("CREATE TYPE r AS ENUM ('G', 'PG-13', 'it''s');");
    assert_eq!(c.get_enum("r").unwrap().values, vec!["G", "PG-13", "it's"]);
}

#[test]
fn fallback_parsing_preserves_order_of_create_and_drop() {
    let c = catalog(
        r#"
CREATE TABLE a (id int);
CREATE FUNCTION f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql;
DROP TABLE a;
CREATE TABLE a (renamed int);
"#,
    );
    assert_eq!(table(&c, "a").column_names(), vec!["renamed"]);
}

// =====================================================================
// 6. Diagnostic quality: E0001 table not found
// =====================================================================

#[test]
fn e0001_message_help_and_span() {
    let c = diag_catalog();
    let diags = analyze(&c, "SELECT 1 FROM userz");
    assert_eq!(diags.len(), 1, "{:?}", diags);
    let d = &diags[0];
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.code(), "E0001");
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.message, "Table 'userz' not found");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'users'?"));
    let diags = analyze(&c, "SELECT 1 FROM zzzzzz");
    assert_eq!(
        diags[0].help.as_deref(),
        Some(
            "Check that the table exists in your schema definition; \
             run `sqlsift schema <schema files>` to list the tables that were loaded"
        )
    );
    assert_eq!(loc(d), (1, 15, 5));
}

#[test]
fn e0001_span_on_later_indented_line() {
    let c = diag_catalog();
    let sql = "SELECT 1;\n\n    SELECT 1\n      FROM userz;";
    let d = single(&c, sql);
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(loc(&d), (4, 12, 5));
}

#[test]
fn e0001_insert_update_delete_spans() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO userz (a) VALUES (1)");
    assert_eq!((d.code(), loc(&d)), ("E0001", (1, 13, 5)));
    let d = single(&c, "UPDATE userz SET a = 1");
    assert_eq!((d.code(), loc(&d)), ("E0001", (1, 8, 5)));
    let diags = analyze(&c, "DELETE FROM userz WHERE id = 1");
    assert_eq!(diags[0].code(), "E0001");
    assert_eq!(loc(&diags[0]), (1, 13, 5));
}

#[test]
fn e0001_unknown_qualifier() {
    let c = diag_catalog();
    let d = single(&c, "SELECT q.id FROM users");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.message, "Table or alias 'q' not found in FROM clause");
    assert_eq!(loc(&d), (1, 8, 1));

    let d = single(&c, "SELECT q.* FROM users");
    assert_eq!(d.message, "Table or alias 'q' not found in FROM clause");
    assert_eq!(loc(&d), (1, 8, 1));
}

#[test]
fn e0001_select_star_without_from() {
    let c = diag_catalog();
    let d = single(&c, "SELECT *");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(
        d.message,
        "SELECT * requires at least one table in FROM clause"
    );
    assert_eq!(loc(&d), (1, 1, 6));
}

#[test]
fn e0001_original_name_hidden_by_alias() {
    // In PostgreSQL, once a table is aliased, the original name is not visible.
    let c = diag_catalog();
    let d = single(&c, "SELECT users.id FROM users u");
    assert_eq!(d.kind, DiagnosticKind::TableNotFound);
    assert_eq!(d.message, "Table or alias 'users' not found in FROM clause");
}

// =====================================================================
// 7. Diagnostic quality: E0002 column not found
// =====================================================================

#[test]
fn e0002_message_help_and_span() {
    let c = diag_catalog();
    let d = single(&c, "SELECT naem FROM users");
    assert_eq!(d.code(), "E0002");
    assert_eq!(d.kind.name(), "column-not-found");
    assert_eq!(d.message, "Column 'naem' not found in table 'users'");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'name'?"));
    assert_eq!(loc(&d), (1, 8, 4));
}

#[test]
fn e0002_qualified_message_includes_table() {
    let c = diag_catalog();
    let d = single(&c, "SELECT o.totl FROM orders o");
    assert_eq!(d.message, "Column 'totl' not found in table 'orders'");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'total'?"));
    assert_eq!(loc(&d), (1, 10, 4));
}

#[test]
fn e0002_without_close_match_has_no_help() {
    let c = diag_catalog();
    let d = single(&c, "SELECT completely_unrelated FROM users");
    assert_eq!(
        d.message,
        "Column 'completely_unrelated' not found in table 'users'"
    );
    assert!(d.help.is_none(), "unexpected help: {:?}", d.help);
}

#[test]
fn e0002_insert_and_update_targets() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO users (nmae) VALUES ('x')");
    assert_eq!(d.message, "Column 'nmae' not found in table 'users'");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'name'?"));
    assert_eq!(loc(&d), (1, 20, 4));

    let d = single(&c, "UPDATE users\n   SET nmae = 'x'");
    assert_eq!(d.message, "Column 'nmae' not found in table 'users'");
    assert_eq!(loc(&d), (2, 8, 4));
}

#[test]
fn e0002_span_second_statement() {
    let c = diag_catalog();
    let d = single(&c, "SELECT 1;\nSELECT naem FROM users;");
    assert_eq!(loc(&d), (2, 8, 4));
}

#[test]
fn e0002_span_in_where_on_third_line() {
    let c = diag_catalog();
    let d = single(&c, "SELECT id\nFROM users\nWHERE nme = 'x'");
    assert_eq!(d.message, "Column 'nme' not found in table 'users'");
    assert_eq!(loc(&d), (3, 7, 3));
}

#[test]
fn e0002_span_after_leading_comments() {
    let c = diag_catalog();
    let d = single(
        &c,
        "-- leading comment\n/* block\ncomment */\nSELECT naem FROM users",
    );
    assert_eq!(loc(&d), (4, 8, 4));
}

#[test]
fn e0002_span_with_crlf_line_endings() {
    let c = diag_catalog();
    let d = single(&c, "SELECT 1;\r\nSELECT naem FROM users;\r\n");
    assert_eq!(loc(&d), (2, 8, 4));
    let d = single(&c, "SELECT id,\r\n       naem\r\nFROM users");
    assert_eq!(loc(&d), (2, 8, 4));
}

#[test]
fn e0002_span_with_tabs_counts_one_column_per_tab() {
    let c = diag_catalog();
    let d = single(&c, "\tSELECT naem FROM users");
    assert_eq!(loc(&d), (1, 9, 4));
    let d = single(&c, "SELECT\tnaem FROM users");
    assert_eq!(loc(&d), (1, 8, 4));
    let d = single(&c, "SELECT id\n\t\tFROM users WHERE naem = 1");
    assert_eq!(loc(&d), (2, 20, 4));
}

#[test]
fn e0002_span_columns_count_characters_not_bytes() {
    let c = diag_catalog();
    // 'é' is 2 bytes, '日本語' is 9 bytes, '😀' is 4 bytes; columns count chars.
    let d = single(&c, "SELECT 'é' AS a, naem FROM users");
    assert_eq!(loc(&d), (1, 18, 4));
    let d = single(&c, "SELECT '日本語' AS a, naem FROM users");
    assert_eq!(loc(&d), (1, 20, 4));
    let d = single(&c, "SELECT '😀' AS a, naem FROM users");
    assert_eq!(loc(&d), (1, 18, 4));
}

#[test]
fn e0002_multibyte_identifier_length_in_chars() {
    let c = diag_catalog();
    let diags = analyze(&c, "SELECT \"名前\" FROM users");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "Column '名前' not found in table 'users'");
    // Two chars + two quotes.
    assert_eq!(loc(&diags[0]), (1, 8, 4));
}

#[test]
fn e0002_multiple_columns_reported_in_source_order() {
    let c = diag_catalog();
    let diags = analyze(
        &c,
        "SELECT zz, yy FROM users, orders WHERE qq = 1 ORDER BY ww",
    );
    let names: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Column 'zz' not found",
            "Column 'yy' not found",
            "Column 'qq' not found",
            "Column 'ww' not found"
        ]
    );
    let cols: Vec<usize> = diags.iter().map(|d| loc(d).1).collect();
    assert_eq!(cols, vec![8, 12, 40, 56]);
}

#[test]
fn e0002_cte_qualified_message() {
    let c = diag_catalog();
    let d = single(
        &c,
        "WITH c AS (SELECT id AS uid FROM users)\nSELECT c.nope FROM c",
    );
    assert_eq!(d.message, "Column 'nope' not found in CTE 'c'");
    assert_eq!(loc(&d), (2, 10, 4));
}

#[test]
fn e0002_derived_table_qualified_message() {
    let c = diag_catalog();
    let d = single(&c, "SELECT s.nope FROM (SELECT id FROM users) s");
    assert_eq!(d.message, "Column 'nope' not found in subquery 's'");
    assert_eq!(loc(&d), (1, 10, 4));
}

// =====================================================================
// 8. Diagnostic quality: E0003 type mismatch
// =====================================================================

#[test]
fn e0003_where_comparison_message_help_span() {
    let c = diag_catalog();
    let d = single(&c, "SELECT id\nFROM users\nWHERE id = 'abc'");
    assert_eq!(d.code(), "E0003");
    assert_eq!(d.message, "Type mismatch: cannot compare integer with text");
    assert_eq!(
        d.help.as_deref(),
        Some("Types are not implicitly compatible. Consider using explicit CAST.")
    );
    // Span covers the left operand.
    assert_eq!(loc(&d), (3, 7, 2));
}

#[test]
fn e0003_arithmetic_on_text_left_and_right() {
    let c = diag_catalog();
    let d = single(&c, "SELECT id FROM users\nWHERE name + 1 > 0");
    assert_eq!(
        d.message,
        "Arithmetic operation requires numeric types, but got varchar(100)"
    );
    assert_eq!(loc(&d), (2, 7, 4));

    let d = single(&c, "SELECT id FROM users\nWHERE 1 + name > 0");
    assert_eq!(loc(&d), (2, 11, 4));
}

#[test]
fn e0003_insert_and_update_messages() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO users (id, name)\nVALUES ('x', 'a')");
    assert_eq!(d.code(), "E0003");
    assert_eq!(
        d.message,
        "Type mismatch: column 'id' expects integer, but got text"
    );
    assert_eq!(
        d.help.as_deref(),
        Some("Value type is not compatible with the column type. Consider using explicit CAST.")
    );

    let d = single(&c, "UPDATE users\n   SET id = 'x'\n WHERE id = 1");
    assert_eq!(
        d.message,
        "Type mismatch: column 'id' expects integer, but got text"
    );
}

#[test]
fn e0003_uses_column_type_from_alter_add_column() {
    let c = catalog("CREATE TABLE t (id int); ALTER TABLE t ADD COLUMN flag boolean;");
    let d = single(&c, "SELECT id FROM t WHERE flag = 'maybe' AND id = 1");
    assert_eq!(d.message, "Type mismatch: cannot compare boolean with text");
    assert_eq!(loc(&d), (1, 24, 4));
}

// =====================================================================
// 9. Diagnostic quality: E0004 / E0005 / E0006 / E0007
// =====================================================================

#[test]
fn e0004_message_and_help() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO users (id, name)\nVALUES (1, NULL)");
    assert_eq!(d.code(), "E0004");
    assert_eq!(
        d.message,
        "Potential NOT NULL violation: column 'name' cannot be assigned NULL"
    );
    assert_eq!(
        d.help.as_deref(),
        Some("This column is defined as NOT NULL. Provide a non-NULL value or change the schema constraint.")
    );
    let d = single(&c, "UPDATE users\n   SET name = NULL");
    assert_eq!(d.code(), "E0004");
}

#[test]
fn e0004_respects_not_null_from_alter_add_column() {
    let c = catalog("CREATE TABLE t (id int); ALTER TABLE t ADD COLUMN must text NOT NULL;");
    assert_codes(&c, "UPDATE t SET must = NULL", &["E0004"]);
    assert_clean(&c, "UPDATE t SET id = NULL");
}

#[test]
fn e0004_respects_primary_key_from_alter_add_constraint() {
    let c = catalog(
        "CREATE TABLE t (id int, n text); ALTER TABLE t ADD CONSTRAINT t_pkey PRIMARY KEY (id);",
    );
    assert_codes(&c, "INSERT INTO t (id, n) VALUES (NULL, 'x')", &["E0004"]);
}

#[test]
fn e0005_message_and_help_with_column_list() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO users (id, name) VALUES (1)");
    assert_eq!(d.code(), "E0005");
    assert_eq!(
        d.message,
        "INSERT has 1 value(s) but 2 column(s) were specified"
    );
    assert_eq!(
        d.help.as_deref(),
        Some("Provide 2 value(s) to match the column list")
    );
}

#[test]
fn e0005_message_and_help_without_column_list() {
    let c = diag_catalog();
    let d = single(&c, "INSERT INTO users VALUES (1, 'a')");
    assert_eq!(
        d.message,
        "INSERT has 2 value(s) but 3 column(s) were specified"
    );
    assert_eq!(
        d.help.as_deref(),
        Some("Table 'users' has 3 columns. Specify columns explicitly or provide 3 values")
    );
}

#[test]
fn e0005_reported_per_bad_row() {
    let c = diag_catalog();
    let diags = analyze(
        &c,
        "INSERT INTO users (id, name) VALUES (1, 'a'), (2), (3, 'c', 'x')",
    );
    let msgs: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        msgs,
        vec![
            "INSERT has 1 value(s) but 2 column(s) were specified",
            "INSERT has 3 value(s) but 2 column(s) were specified"
        ]
    );
}

#[test]
fn e0005_counts_columns_added_by_migration() {
    let c = catalog("CREATE TABLE t (a int); ALTER TABLE t ADD COLUMN b int;");
    assert_clean(&c, "INSERT INTO t VALUES (1, 2)");
    let d = single(&c, "INSERT INTO t VALUES (1)");
    assert!(d.help.as_deref().unwrap().contains("has 2 columns"));
}

#[test]
fn e0006_message_help_and_span() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT id\n  FROM users u\n  JOIN orders o ON o.user_id = u.id",
    );
    assert_eq!(d.code(), "E0006");
    assert_eq!(d.kind.name(), "ambiguous-column");
    // NOTE: table order in the message is not deterministic (see report), so
    // both permutations are accepted here.
    assert!(
        d.message == "Column 'id' is ambiguous (found in tables: u, o)"
            || d.message == "Column 'id' is ambiguous (found in tables: o, u)",
        "unexpected message: {}",
        d.message
    );
    let help = d.help.as_deref().unwrap();
    assert!(
        help == "Qualify the column with a table name: u.id"
            || help == "Qualify the column with a table name: o.id",
        "unexpected help: {}",
        help
    );
    assert_eq!(loc(&d), (1, 8, 2));
}

#[test]
fn e0006_in_where_clause_span() {
    let c = diag_catalog();
    let d = single(&c, "SELECT u.name FROM users u, orders o\nWHERE id = 1");
    assert_eq!(d.kind, DiagnosticKind::AmbiguousColumn);
    assert_eq!(loc(&d), (2, 7, 2));
}

#[test]
fn e0007_message_help_span() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT u.id FROM users u\n  JOIN orders o ON u.name = o.user_id",
    );
    assert_eq!(d.code(), "E0007");
    assert_eq!(
        d.message,
        "JOIN condition type mismatch: varchar(100) vs integer"
    );
    assert_eq!(
        d.help.as_deref(),
        Some("JOIN condition should compare compatible types. Consider using explicit CAST.")
    );
    // Span covers the full left compound identifier `u.name`.
    assert_eq!(loc(&d), (2, 20, 6));
}

#[test]
fn e0007_operand_order_reflected_in_message() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT u.id FROM users u JOIN orders o ON o.user_id = u.name",
    );
    assert_eq!(
        d.message,
        "JOIN condition type mismatch: integer vs varchar(100)"
    );
    assert_eq!(loc(&d), (1, 43, 9));
}

// =====================================================================
// 10. Parse errors (E1000)
// =====================================================================

#[test]
fn e1000_parse_error_message_contains_location() {
    let c = diag_catalog();
    let d = single(&c, "SELECT FROM WHERE");
    assert_eq!(d.code(), "E1000");
    assert_eq!(d.kind, DiagnosticKind::ParseError);
    assert!(d.message.starts_with("Parse error: "), "{}", d.message);
    assert!(!d.message.contains("sql parser error"), "{}", d.message);
    // The parser location is the diagnostic's span rather than part of the message
    let span = d.span.unwrap();
    assert_eq!((span.line, span.column), (1, 13));
}

#[test]
fn e1000_parse_error_in_one_statement_does_not_hide_other_diagnostics() {
    // Statements are parsed one by one when the input doesn't parse as a whole
    let c = diag_catalog();
    let diags = analyze(
        &c,
        "SELECT naem FROM users;\nSELECT FROM WHERE;\nSELECT id FROM userz;",
    );
    assert_eq!(
        codes(&diags),
        vec!["E0002", "E1000", "E0001"],
        "{:?}",
        diags
    );
    let span = diags[1].span.unwrap();
    assert_eq!((span.line, span.column), (2, 13));
}

#[test]
fn e1000_misspelled_keyword_on_later_line() {
    let c = diag_catalog();
    let d = single(&c, "SELECT id FROM users;\n\nSELEC id FROM users;");
    assert_eq!(d.kind, DiagnosticKind::ParseError);
    assert!(d.message.contains("SELEC"), "{}", d.message);
    let span = d.span.unwrap();
    assert_eq!((span.line, span.column), (3, 1));
}

#[test]
fn e1000_unexpected_eof() {
    let c = diag_catalog();
    let d = single(&c, "SELECT id FROM users WHERE");
    assert_eq!(d.kind, DiagnosticKind::ParseError);
    assert!(d.message.contains("EOF"), "{}", d.message);
}

#[test]
fn empty_and_comment_only_queries_have_no_diagnostics() {
    let c = diag_catalog();
    assert_clean(&c, "");
    assert_clean(&c, "   \n\t ");
    assert_clean(&c, "-- nothing here\n/* or here */");
    assert_clean(&c, ";");
}

#[test]
fn ddl_in_query_file_is_not_analyzed() {
    let c = diag_catalog();
    assert_clean(&c, "CREATE TABLE whatever (x int); SELECT id FROM users;");
}

// =====================================================================
// 11. Ordering and determinism
// =====================================================================

#[test]
fn diagnostics_ordered_by_statement() {
    let c = diag_catalog();
    let diags = analyze(
        &c,
        "SELECT id FROM users;\nSELECT naem FROM users WHERE id = 'x';\nINSERT INTO orders (id) VALUES (1, 2);\nSELECT 1 FROM nope;",
    );
    assert_eq!(codes(&diags), vec!["E0002", "E0003", "E0005", "E0001"]);
    assert_eq!(diags[0].span.unwrap().line, 2);
    assert_eq!(diags[1].span.unwrap().line, 2);
    assert_eq!(diags[3].span.unwrap().line, 4);
}

#[test]
fn diagnostics_within_a_statement_are_in_source_order() {
    let c = diag_catalog();
    let diags = analyze(&c, "SELECT id FROM users WHERE id = 'x' AND nme = 'y'");
    assert_eq!(codes(&diags), vec!["E0003", "E0002"]);
}

#[test]
fn repeated_analysis_is_stable() {
    let c = diag_catalog();
    let sql = "SELECT zz FROM users;\nSELECT o.qq FROM orders o WHERE o.id = 'x';\nINSERT INTO users (nmae) VALUES (1, 2);";
    let render = |diags: Vec<Diagnostic>| -> Vec<String> {
        diags
            .iter()
            .map(|d| format!("{} {} {:?} {:?}", d.code(), d.message, d.help, d.span))
            .collect()
    };
    let first = render(analyze(&c, sql));
    assert_eq!(first.len(), 5, "{:#?}", first);
    for _ in 0..20 {
        assert_eq!(render(analyze(&c, sql)), first);
    }
}

#[test]
fn analyzer_instance_reuse_clears_previous_diagnostics() {
    let c = diag_catalog();
    let mut analyzer = Analyzer::new(&c);
    assert_eq!(analyzer.analyze("SELECT naem FROM users").len(), 1);
    assert!(analyzer.analyze("SELECT name FROM users").is_empty());
    assert_eq!(analyzer.analyze("SELECT 1 FROM nope").len(), 1);
}

#[test]
fn name_resolution_spans_are_one_indexed() {
    let c = diag_catalog();
    let sql = "SELECT naem FROM users;\nSELECT u.x FROM users u;\nSELECT * FROM nope;\nSELECT id FROM users, orders;\nSELECT 1 FROM users WHERE name + 1 = 2;";
    let diags = analyze(&c, sql);
    assert!(diags.len() >= 5, "{:?}", diags);
    for d in &diags {
        let s = d.span.expect("span");
        assert!(s.line >= 1 && s.column >= 1, "bad span for {:?}", d);
        assert!(s.length >= 1);
    }
}

// =====================================================================
// 12. Inline suppression
// =====================================================================

#[test]
fn suppress_same_line() {
    let c = diag_catalog();
    assert_clean(&c, "SELECT naem FROM users -- sqlsift:disable E0002");
}

#[test]
fn suppress_next_line() {
    let c = diag_catalog();
    assert_clean(&c, "-- sqlsift:disable E0002\nSELECT naem FROM users");
}

#[test]
fn suppress_next_line_does_not_leak_to_following_statement() {
    let c = diag_catalog();
    let d = single(
        &c,
        "-- sqlsift:disable E0002\nSELECT naem FROM users;\nSELECT naem FROM users",
    );
    assert_eq!(loc(&d).0, 3);
}

#[test]
fn suppress_next_line_only_covers_one_line_of_multiline_statement() {
    let c = diag_catalog();
    let d = single(
        &c,
        "-- sqlsift:disable E0002\nSELECT id\nFROM users WHERE naem = 1",
    );
    assert_eq!(loc(&d), (3, 18, 4));
}

#[test]
fn suppress_wrong_code_does_not_suppress() {
    let c = diag_catalog();
    let d = single(&c, "-- sqlsift:disable E0001\nSELECT naem FROM users");
    assert_eq!(d.code(), "E0002");
}

#[test]
fn suppress_multiple_codes_comma_and_space_separated() {
    let c = diag_catalog();
    assert_clean(
        &c,
        "SELECT naem, id FROM userz -- sqlsift:disable E0001, E0002",
    );
    assert_clean(
        &c,
        "SELECT naem, id FROM userz -- sqlsift:disable E0001 E0002",
    );
    assert_clean(
        &c,
        "SELECT naem, id FROM userz -- sqlsift:disable E0001,E0002",
    );
}

#[test]
fn suppress_bare_disable_suppresses_all_codes_on_line() {
    let c = diag_catalog();
    assert_clean(&c, "SELECT naem FROM userz -- sqlsift:disable");
    assert_clean(&c, "-- sqlsift:disable\nSELECT naem FROM userz");
}

#[test]
fn suppress_accumulates_stacked_directives() {
    let c = diag_catalog();
    assert_clean(
        &c,
        "-- sqlsift:disable E0001\n-- sqlsift:disable E0002\nSELECT naem FROM userz",
    );
}

#[test]
fn suppress_skips_blank_and_comment_lines() {
    let c = diag_catalog();
    assert_clean(&c, "-- sqlsift:disable E0002\n\n\nSELECT naem FROM users");
    assert_clean(
        &c,
        "-- sqlsift:disable E0002\n-- an ordinary comment\nSELECT naem FROM users",
    );
}

#[test]
fn suppress_same_line_only_affects_that_line() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT\n  naem, -- sqlsift:disable E0002\n  emial\nFROM users",
    );
    assert_eq!(d.message, "Column 'emial' not found in table 'users'");
    assert_eq!(d.help.as_deref(), Some("Did you mean 'email'?"));
    assert_eq!(loc(&d), (3, 3, 5));
}

#[test]
fn suppress_same_line_does_not_cover_next_line() {
    let c = diag_catalog();
    let d = single(&c, "SELECT naem -- sqlsift:disable E0002\nFROM userz");
    assert_eq!(d.code(), "E0001");
    assert_eq!(loc(&d), (2, 6, 5));
}

#[test]
fn suppress_with_crlf_line_endings() {
    let c = diag_catalog();
    assert_clean(
        &c,
        "SELECT id FROM users\r\n-- sqlsift:disable E0002\r\nWHERE naem = 1",
    );
}

#[test]
fn suppress_code_case_insensitive_and_extra_text() {
    let c = diag_catalog();
    assert_clean(&c, "SELECT naem FROM users -- sqlsift:disable e0002");
    assert_clean(&c, "SELECT naem FROM users --sqlsift:disable E0002");
    assert_clean(
        &c,
        "SELECT naem FROM users -- sqlsift:disable E0002 legacy column",
    );
    assert_clean(&c, "SELECT naem FROM users -- sqlsift:disable E0002,");
}

#[test]
fn suppress_accepts_rule_names() {
    let c = diag_catalog();
    assert_clean(
        &c,
        "SELECT naem FROM users -- sqlsift:disable column-not-found",
    );
    // A different rule's name doesn't suppress
    assert_codes(
        &c,
        "SELECT naem FROM users -- sqlsift:disable table-not-found",
        &["E0002"],
    );
}

#[test]
fn suppress_does_not_accept_uppercase_prefix() {
    // The directive prefix is case-sensitive.
    let c = diag_catalog();
    assert_codes(
        &c,
        "SELECT naem FROM users -- SQLSIFT:DISABLE E0002",
        &["E0002"],
    );
    assert_codes(
        &c,
        "SELECT naem FROM users -- sqlsift:disabled E0002",
        &["E0002"],
    );
}

#[test]
fn suppress_block_comments_are_not_directives() {
    // Current (documented) behavior: only `--` line comments are directives.
    let c = diag_catalog();
    assert_codes(
        &c,
        "SELECT naem FROM users /* sqlsift:disable E0002 */",
        &["E0002"],
    );
    assert_codes(
        &c,
        "/* sqlsift:disable E0002 */\nSELECT naem FROM users",
        &["E0002"],
    );
}

#[test]
fn suppress_directive_text_inside_string_literal_is_ignored() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT\n  'a -- sqlsift:disable E0002' AS s,\n  naem\nFROM users",
    );
    assert_eq!(loc(&d), (3, 3, 4));
    let d = single(
        &c,
        "SELECT 'x -- sqlsift:disable E0002' AS s, naem FROM users",
    );
    assert_eq!(d.code(), "E0002");
}

#[test]
fn suppress_where_comparison_type_mismatch() {
    let c = diag_catalog();
    assert_clean(
        &c,
        "SELECT id FROM users\nWHERE id = 'x' -- sqlsift:disable E0003",
    );
}

#[test]
fn suppress_ambiguous_and_join_mismatch() {
    let c = diag_catalog();
    assert_clean(&c, "SELECT id FROM users, orders -- sqlsift:disable E0006");
    assert_clean(
        &c,
        "SELECT u.id FROM users u\n  -- sqlsift:disable E0007\n  JOIN orders o ON u.name = o.user_id",
    );
}

#[test]
fn suppress_one_of_two_errors_on_same_line() {
    let c = diag_catalog();
    let d = single(
        &c,
        "SELECT naem FROM users WHERE id = 'x' -- sqlsift:disable E0002",
    );
    assert_eq!(d.code(), "E0003");
}

#[test]
fn suppress_in_middle_statement_of_file() {
    let c = diag_catalog();
    let diags = analyze(
        &c,
        "SELECT naem FROM users;\n-- sqlsift:disable E0002\nSELECT naem FROM users;\nSELECT naem FROM users;",
    );
    let lines: Vec<usize> = diags.iter().map(|d| d.span.unwrap().line).collect();
    assert_eq!(lines, vec![1, 4]);
}
