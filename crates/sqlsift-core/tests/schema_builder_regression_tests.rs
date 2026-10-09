//! Regression tests for schema building (DDL -> Catalog).
//!
//! Each section covers a bug found by DDL test sweeps: statements that were
//! ignored or mis-applied (IF NOT EXISTS, ALTER COLUMN, DROP VIEW, ALTER TYPE,
//! LIKE / CTAS), view column inference, inline constraints, warnings for
//! skipped statements, the fallback statement splitter and type mappings.

use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::{Diagnostic, DiagnosticKind, Severity};
use sqlsift_core::schema::{Catalog, DefaultValue, QualifiedName, SchemaBuilder, TableDef};
use sqlsift_core::types::SqlType;

// =====================================================================
// Helpers
// =====================================================================

/// Build a catalog with the given dialect, returning warnings too.
fn build(dialect: SqlDialect, schema: &str) -> (Catalog, Vec<Diagnostic>) {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    let result = builder.parse(schema);
    assert!(
        result.is_ok(),
        "schema parse returned errors: {:?}",
        result.err()
    );
    builder.build()
}

/// Build a PostgreSQL catalog, asserting there are no warnings.
#[track_caller]
fn pg(schema: &str) -> Catalog {
    let (catalog, warnings) = build(SqlDialect::PostgreSQL, schema);
    assert!(
        warnings.is_empty(),
        "expected no schema warnings, got: {warnings:#?}"
    );
    catalog
}

/// Build a MySQL catalog, asserting there are no warnings.
#[track_caller]
fn mysql(schema: &str) -> Catalog {
    let (catalog, warnings) = build(SqlDialect::MySQL, schema);
    assert!(
        warnings.is_empty(),
        "expected no schema warnings, got: {warnings:#?}"
    );
    catalog
}

#[track_caller]
fn table<'a>(catalog: &'a Catalog, name: &str) -> &'a TableDef {
    catalog
        .get_table(&QualifiedName::parse(name))
        .unwrap_or_else(|| {
            panic!(
                "table {name:?} missing from catalog; tables = {:?}",
                catalog.table_names()
            )
        })
}

#[track_caller]
fn view_columns(catalog: &Catalog, name: &str) -> Vec<String> {
    catalog
        .get_view(&QualifiedName::parse(name))
        .unwrap_or_else(|| panic!("view {name:?} missing from catalog"))
        .columns
        .clone()
}

#[track_caller]
fn column_type(catalog: &Catalog, table_name: &str, column: &str) -> SqlType {
    table(catalog, table_name)
        .get_column(column)
        .unwrap_or_else(|| panic!("column {table_name}.{column} missing"))
        .data_type
        .clone()
}

#[track_caller]
fn nullable(catalog: &Catalog, table_name: &str, column: &str) -> bool {
    table(catalog, table_name)
        .get_column(column)
        .unwrap_or_else(|| panic!("column {table_name}.{column} missing"))
        .nullable
}

fn codes(diags: &[Diagnostic]) -> Vec<&'static str> {
    diags.iter().map(|d| d.code()).collect()
}

#[track_caller]
fn assert_codes(catalog: &Catalog, dialect: SqlDialect, sql: &str, expected: &[&str]) {
    let diags = Analyzer::with_dialect(catalog, dialect).analyze(sql);
    assert_eq!(
        codes(&diags),
        expected,
        "unexpected diagnostic codes for {sql:?}: {diags:#?}"
    );
}

#[track_caller]
fn assert_clean(catalog: &Catalog, sql: &str) {
    assert_codes(catalog, SqlDialect::PostgreSQL, sql, &[]);
}

// =====================================================================
// 1. CREATE TABLE IF NOT EXISTS keeps an existing table
// =====================================================================

#[test]
fn create_table_if_not_exists_keeps_existing_table() {
    let c = pg("CREATE TABLE t (a int, b text);
         CREATE TABLE IF NOT EXISTS t (x int);");
    assert_eq!(table(&c, "t").column_names(), vec!["a", "b"]);
    assert_clean(&c, "SELECT a, b FROM t");
    assert_codes(&c, SqlDialect::PostgreSQL, "SELECT x FROM t", &["E0002"]);
}

#[test]
fn create_table_if_not_exists_matches_folded_name() {
    let c = pg("CREATE TABLE Users (id int);
         CREATE TABLE IF NOT EXISTS users (other int);");
    assert_eq!(table(&c, "users").column_names(), vec!["id"]);
}

#[test]
fn create_table_if_not_exists_keeps_existing_mysql_table() {
    let c = mysql(
        "CREATE TABLE `t` (`a` INT NOT NULL);
         CREATE TABLE IF NOT EXISTS `t` (`z` INT);",
    );
    assert_eq!(table(&c, "t").column_names(), vec!["a"]);
}

// =====================================================================
// 2. ALTER TABLE ... ALTER COLUMN / MODIFY / CHANGE
// =====================================================================

#[test]
fn alter_column_type_is_applied() {
    let c = pg("CREATE TABLE t (a int, b int, c varchar(10));
         ALTER TABLE t ALTER COLUMN a TYPE bigint USING a::bigint;
         ALTER TABLE t ALTER COLUMN b SET DATA TYPE text;
         ALTER TABLE ONLY public.t ALTER c TYPE varchar(200);");
    assert_eq!(column_type(&c, "t", "a"), SqlType::BigInt);
    assert_eq!(column_type(&c, "t", "b"), SqlType::Text);
    assert_eq!(
        column_type(&c, "t", "c"),
        SqlType::Varchar { length: Some(200) }
    );
    assert_eq!(table(&c, "t").column_names(), vec!["a", "b", "c"]);
}

#[test]
fn alter_column_nullability_is_applied() {
    let c = pg("CREATE TABLE t (a int, b int NOT NULL);
         ALTER TABLE t ALTER COLUMN a SET NOT NULL;
         ALTER TABLE t ALTER COLUMN b DROP NOT NULL;");
    assert!(!nullable(&c, "t", "a"));
    assert!(nullable(&c, "t", "b"));
    assert_codes(
        &c,
        SqlDialect::PostgreSQL,
        "UPDATE t SET a = NULL",
        &["E0004"],
    );
    assert_clean(&c, "UPDATE t SET b = NULL");
}

#[test]
fn alter_column_default_is_applied() {
    let c = pg("CREATE TABLE t (a int DEFAULT 1, b int);
         ALTER TABLE t ALTER COLUMN a DROP DEFAULT;
         ALTER TABLE t ALTER COLUMN b SET DEFAULT 42;");
    let t = table(&c, "t");
    assert!(t.get_column("a").unwrap().default.is_none());
    assert!(matches!(
        t.get_column("b").unwrap().default,
        Some(DefaultValue::Literal(ref v)) if v == "42"
    ));
}

#[test]
fn alter_column_matches_column_case_insensitively() {
    let c = pg("CREATE TABLE t (Amount int);
         ALTER TABLE t ALTER COLUMN amount TYPE numeric(10,2);");
    assert_eq!(
        column_type(&c, "t", "amount"),
        SqlType::Decimal {
            precision: Some(10),
            scale: Some(2)
        }
    );
    assert_eq!(table(&c, "t").columns.len(), 1);
}

#[test]
fn alter_column_on_missing_table_warns() {
    let (_, warnings) = build(
        SqlDialect::PostgreSQL,
        "ALTER TABLE missing ALTER COLUMN a SET NOT NULL;",
    );
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].message.contains("missing"));
}

#[test]
fn mysql_modify_column_is_applied() {
    let c = mysql(
        "CREATE TABLE t (a INT, b VARCHAR(10) NOT NULL, c INT);
         ALTER TABLE t MODIFY COLUMN a BIGINT NOT NULL;
         ALTER TABLE t MODIFY b TEXT;",
    );
    assert_eq!(column_type(&c, "t", "a"), SqlType::BigInt);
    assert!(!nullable(&c, "t", "a"));
    assert_eq!(column_type(&c, "t", "b"), SqlType::Text);
    // MODIFY replaces the definition: without NOT NULL the column is nullable
    assert!(nullable(&c, "t", "b"));
    assert_eq!(table(&c, "t").column_names(), vec!["a", "b", "c"]);
}

#[test]
fn mysql_change_column_renames_and_retypes() {
    let c = mysql(
        "CREATE TABLE t (a INT, b INT, c INT);
         ALTER TABLE t CHANGE COLUMN b renamed BIGINT NOT NULL DEFAULT 0;",
    );
    // Position is kept
    assert_eq!(table(&c, "t").column_names(), vec!["a", "renamed", "c"]);
    assert_eq!(column_type(&c, "t", "renamed"), SqlType::BigInt);
    assert!(!nullable(&c, "t", "renamed"));
    assert_codes(&c, SqlDialect::MySQL, "SELECT renamed FROM t", &[]);
    assert_codes(&c, SqlDialect::MySQL, "SELECT b FROM t", &["E0002"]);
}

#[test]
fn rename_column_updates_constraints() {
    let c = pg("CREATE TABLE t (id int PRIMARY KEY, code text UNIQUE);
         ALTER TABLE t RENAME COLUMN id TO t_id;");
    let t = table(&c, "t");
    assert_eq!(t.primary_key.as_ref().unwrap().columns, vec!["t_id"]);
}

// =====================================================================
// 3. DROP VIEW / DROP TABLE
// =====================================================================

#[test]
fn drop_view_removes_view() {
    let c = pg("CREATE TABLE t (a int);
         CREATE VIEW v AS SELECT a FROM t;
         CREATE VIEW w AS SELECT a FROM t;
         DROP VIEW v;");
    assert!(!c.view_exists(&QualifiedName::new("v")));
    assert!(c.view_exists(&QualifiedName::new("w")));
    assert_codes(&c, SqlDialect::PostgreSQL, "SELECT a FROM v", &["E0001"]);
}

#[test]
fn drop_view_if_exists_multiple_names() {
    let c = pg("CREATE TABLE t (a int);
         CREATE VIEW v1 AS SELECT a FROM t;
         CREATE VIEW v2 AS SELECT a FROM t;
         DROP VIEW IF EXISTS v1, V2, nonexistent CASCADE;");
    assert!(!c.view_exists(&QualifiedName::new("v1")));
    assert!(!c.view_exists(&QualifiedName::new("v2")));
}

#[test]
fn drop_view_schema_qualified() {
    let c = pg("CREATE TABLE t (a int);
         CREATE VIEW public.v AS SELECT a FROM t;
         DROP VIEW public.v;");
    assert!(!c.view_exists(&QualifiedName::new("v")));
}

#[test]
fn drop_table_multiple_names_and_if_exists() {
    let c = pg("CREATE TABLE a (id int);
         CREATE TABLE b (id int);
         CREATE TABLE keep (id int);
         DROP TABLE a, B;
         DROP TABLE IF EXISTS missing, keep2;");
    assert!(!c.table_exists(&QualifiedName::new("a")));
    assert!(!c.table_exists(&QualifiedName::new("b")));
    assert!(c.table_exists(&QualifiedName::new("keep")));
}

#[test]
fn recreate_view_after_drop() {
    let c = pg("CREATE TABLE t (a int, b int);
         CREATE VIEW v AS SELECT a FROM t;
         DROP VIEW v;
         CREATE VIEW v AS SELECT b FROM t;");
    assert_eq!(view_columns(&c, "v"), vec!["b"]);
}

// =====================================================================
// 4. ALTER TYPE ... ADD VALUE / RENAME VALUE
// =====================================================================

#[test]
fn alter_type_add_value_updates_enum() {
    let c = pg("CREATE TYPE mood AS ENUM ('sad', 'happy');
         ALTER TYPE mood ADD VALUE 'ecstatic';
         ALTER TYPE mood ADD VALUE IF NOT EXISTS 'sad';
         ALTER TYPE public.mood ADD VALUE 'meh' BEFORE 'happy';
         ALTER TYPE mood ADD VALUE 'blue' AFTER 'sad';
         CREATE TABLE t (m mood);");
    assert_eq!(
        c.get_enum("mood").unwrap().values,
        vec!["sad", "blue", "meh", "happy", "ecstatic"]
    );
    assert!(c.table_exists(&QualifiedName::new("t")));
}

#[test]
fn alter_type_rename_value_updates_enum() {
    let c = pg("CREATE TYPE mood AS ENUM ('sad', 'happy');
         ALTER TYPE mood RENAME VALUE 'sad' TO 'unhappy';");
    assert_eq!(c.get_enum("mood").unwrap().values, vec!["unhappy", "happy"]);
}

#[test]
fn alter_type_other_forms_are_skipped_silently() {
    let c = pg("CREATE TYPE mood AS ENUM ('sad');
         ALTER TYPE mood OWNER TO postgres;
         CREATE TABLE t (id int);");
    assert_eq!(c.get_enum("mood").unwrap().values, vec!["sad"]);
    assert!(c.table_exists(&QualifiedName::new("t")));
}

// =====================================================================
// 5. CREATE TABLE ... LIKE / CREATE TABLE AS SELECT
// =====================================================================

#[test]
fn create_table_like_in_column_list_copies_columns() {
    let c = pg("CREATE TABLE p (id int NOT NULL, name text);
         CREATE TABLE c (LIKE p);");
    assert_eq!(table(&c, "c").column_names(), vec!["id", "name"]);
    assert!(!nullable(&c, "c", "id"));
    assert_clean(&c, "SELECT id, name FROM c");
}

#[test]
fn create_table_like_with_extra_columns() {
    let c = pg("CREATE TABLE p (id int, name text);
         CREATE TABLE c (LIKE p, extra boolean);");
    assert_eq!(table(&c, "c").column_names(), vec!["id", "name", "extra"]);
}

#[test]
fn create_table_like_including_all() {
    let c = pg("CREATE TABLE p (id int, name text);
         CREATE TABLE c (LIKE p INCLUDING ALL);
         CREATE TABLE d (LIKE p INCLUDING DEFAULTS EXCLUDING CONSTRAINTS, x int);");
    assert_eq!(table(&c, "c").column_names(), vec!["id", "name"]);
    assert_eq!(table(&c, "d").column_names(), vec!["id", "name", "x"]);
}

#[test]
fn mysql_create_table_like_copies_columns() {
    let c = mysql(
        "CREATE TABLE t (id INT NOT NULL AUTO_INCREMENT, name VARCHAR(10), PRIMARY KEY (id));
         CREATE TABLE t2 LIKE t;",
    );
    assert_eq!(table(&c, "t2").column_names(), vec!["id", "name"]);
    assert_eq!(
        table(&c, "t2").primary_key.as_ref().unwrap().columns,
        vec!["id"]
    );
    assert_codes(&c, SqlDialect::MySQL, "SELECT id, name FROM t2", &[]);
}

#[test]
fn create_table_as_select_infers_columns() {
    let c = pg("CREATE TABLE src (id int, name text);
         CREATE TABLE dst AS SELECT id, upper(name), name AS label, count(*) FROM src GROUP BY id, name;");
    assert_eq!(
        table(&c, "dst").column_names(),
        vec!["id", "upper", "label", "count"]
    );
    assert_clean(&c, "SELECT id, upper, label, count FROM dst");
    assert_codes(
        &c,
        SqlDialect::PostgreSQL,
        "SELECT nope FROM dst",
        &["E0002"],
    );
}

#[test]
fn create_table_as_select_star() {
    let c = pg("CREATE TABLE src (id int, name text);
         CREATE TABLE dst AS SELECT * FROM src;
         CREATE TABLE empty_copy AS SELECT id FROM src WITH NO DATA;");
    assert_eq!(table(&c, "dst").column_names(), vec!["id", "name"]);
    assert_eq!(table(&c, "empty_copy").column_names(), vec!["id"]);
}

#[test]
fn materialized_view_with_no_data_is_created() {
    // pg_dump emits materialized views with a trailing WITH NO DATA
    let c = pg("CREATE TABLE t (id int, amount numeric);
         CREATE MATERIALIZED VIEW public.totals AS
          SELECT t.id, sum(t.amount) AS total
            FROM public.t
           GROUP BY t.id
          WITH NO DATA;
         CREATE MATERIALIZED VIEW m2 AS SELECT id FROM t WITH DATA;");
    assert_eq!(view_columns(&c, "totals"), vec!["id", "total"]);
    assert_eq!(view_columns(&c, "m2"), vec!["id"]);
    assert!(
        c.get_view(&QualifiedName::new("totals"))
            .unwrap()
            .materialized
    );
}

#[test]
fn drop_materialized_view_removes_view() {
    let c = pg("CREATE TABLE t (id int);
         CREATE MATERIALIZED VIEW m AS SELECT id FROM t;
         DROP MATERIALIZED VIEW IF EXISTS m;");
    assert!(!c.view_exists(&QualifiedName::new("m")));
}

// =====================================================================
// 6. View column inference
// =====================================================================

#[test]
fn view_implicit_column_names_follow_postgres() {
    let c = pg("CREATE TABLE t (n text, x text, col int);
         CREATE VIEW v AS SELECT upper(n), count(*), t.col, x::int, 1 + 1 FROM t GROUP BY n, t.col, x;");
    assert_eq!(
        view_columns(&c, "v"),
        vec!["upper", "count", "col", "x", "?column?"]
    );
    assert_clean(&c, "SELECT upper, count, col, x FROM v");
}

#[test]
fn view_over_union_uses_left_branch() {
    let c = pg("CREATE TABLE a (id int, name text);
         CREATE TABLE b (bid int, bname text);
         CREATE VIEW v AS SELECT id, name FROM a UNION ALL SELECT bid, bname FROM b;
         CREATE VIEW w AS (SELECT id FROM a) EXCEPT (SELECT bid FROM b);");
    assert_eq!(view_columns(&c, "v"), vec!["id", "name"]);
    assert_eq!(view_columns(&c, "w"), vec!["id"]);
    assert_clean(&c, "SELECT id, name FROM v");
}

#[test]
fn view_qualified_wildcard_resolves_alias() {
    let c = pg("CREATE TABLE u (id int, name text);
         CREATE TABLE o (oid int, uid int);
         CREATE VIEW v AS SELECT x.*, y.oid FROM u x JOIN o y ON y.uid = x.id;");
    assert_eq!(view_columns(&c, "v"), vec!["id", "name", "oid"]);
}

#[test]
fn view_wildcard_includes_joined_tables() {
    let c = pg("CREATE TABLE u (id int, name text);
         CREATE TABLE o (oid int, uid int, total int);
         CREATE VIEW v AS SELECT * FROM u JOIN o ON o.uid = u.id;");
    assert_eq!(
        view_columns(&c, "v"),
        vec!["id", "name", "oid", "uid", "total"]
    );
    assert_clean(&c, "SELECT name, total FROM v");
}

#[test]
fn view_wildcard_with_using_join_lists_column_once() {
    let c = pg("CREATE TABLE a (id int, x int);
         CREATE TABLE b (id int, y int);
         CREATE VIEW v AS SELECT * FROM a JOIN b USING (id);");
    assert_eq!(view_columns(&c, "v"), vec!["id", "x", "y"]);
}

#[test]
fn view_wildcard_with_natural_join_and_qualified_wildcard() {
    let c = pg("CREATE TABLE a (id int, x int);
         CREATE TABLE b (id int, y int);
         CREATE VIEW v AS SELECT * FROM a NATURAL JOIN b;
         CREATE VIEW w AS SELECT b.* FROM a JOIN b USING (id);");
    assert_eq!(view_columns(&c, "v"), vec!["id", "x", "y"]);
    // A qualified wildcard still lists the join column
    assert_eq!(view_columns(&c, "w"), vec!["id", "y"]);
}

#[test]
fn view_wildcard_over_derived_table() {
    let c = pg("CREATE TABLE t (id int, name text);
         CREATE VIEW v AS SELECT * FROM (SELECT id, upper(name) AS uname FROM t) s;
         CREATE VIEW w AS SELECT s.* FROM (SELECT id FROM t) AS s;
         CREATE VIEW z AS SELECT * FROM (SELECT id, name FROM t) AS s(a, b);");
    assert_eq!(view_columns(&c, "v"), vec!["id", "uname"]);
    assert_eq!(view_columns(&c, "w"), vec!["id"]);
    assert_eq!(view_columns(&c, "z"), vec!["a", "b"]);
}

#[test]
fn view_on_view_and_cte() {
    let c = pg("CREATE TABLE t (id int, name text);
         CREATE VIEW v1 AS SELECT id, name FROM t;
         CREATE VIEW v2 AS SELECT * FROM v1;
         CREATE VIEW v3 AS SELECT v.* FROM v2 v;
         CREATE VIEW v4 AS WITH c AS (SELECT id AS cid FROM t) SELECT * FROM c;");
    assert_eq!(view_columns(&c, "v2"), vec!["id", "name"]);
    assert_eq!(view_columns(&c, "v3"), vec!["id", "name"]);
    assert_eq!(view_columns(&c, "v4"), vec!["cid"]);
}

#[test]
fn view_with_unknown_columns_is_empty() {
    let c = pg("CREATE VIEW v AS SELECT * FROM generate_series(1, 3);
         CREATE VIEW w AS SELECT * FROM not_a_table;");
    assert!(view_columns(&c, "v").is_empty());
    assert!(view_columns(&c, "w").is_empty());
}

#[test]
fn query_against_view_with_unknown_columns_is_not_validated() {
    let c = pg("CREATE VIEW v AS SELECT * FROM generate_series(1, 3);");
    assert_clean(&c, "SELECT generate_series FROM v");
    assert_clean(&c, "SELECT v.anything FROM v");
}

// =====================================================================
// 7. Inline column constraints and serial columns
// =====================================================================

#[test]
fn inline_primary_key_is_recorded_on_table() {
    let c = pg("CREATE TABLE t (id int CONSTRAINT t_pk PRIMARY KEY, name text);");
    let pk = table(&c, "t").primary_key.as_ref().expect("primary key");
    assert_eq!(pk.columns, vec!["id"]);
    assert_eq!(pk.name.as_deref(), Some("t_pk"));
}

#[test]
fn inline_unique_is_recorded_on_table() {
    let c = pg("CREATE TABLE t (id int, email text UNIQUE);");
    let t = table(&c, "t");
    assert_eq!(t.unique_constraints.len(), 1);
    assert_eq!(t.unique_constraints[0].columns, vec!["email"]);
}

#[test]
fn inline_references_is_recorded_on_table() {
    let c = pg("CREATE TABLE p (id int PRIMARY KEY);
         CREATE TABLE t (id int, p_id int REFERENCES p(id), q_id int REFERENCES P);");
    let t = table(&c, "t");
    assert_eq!(t.foreign_keys.len(), 2);
    assert_eq!(t.foreign_keys[0].columns, vec!["p_id"]);
    assert_eq!(t.foreign_keys[0].references_table, QualifiedName::new("p"));
    assert_eq!(t.foreign_keys[0].references_columns, vec!["id"]);
    assert_eq!(t.foreign_keys[1].columns, vec!["q_id"]);
    assert_eq!(t.foreign_keys[1].references_table, QualifiedName::new("p"));
    assert!(t.foreign_keys[1].references_columns.is_empty());
}

#[test]
fn alter_add_column_inline_constraints_are_recorded() {
    let c = pg("CREATE TABLE p (id int PRIMARY KEY);
         CREATE TABLE t (x int);
         ALTER TABLE t ADD COLUMN id int PRIMARY KEY;
         ALTER TABLE t ADD COLUMN p_id int REFERENCES p(id);
         ALTER TABLE t ADD COLUMN code text UNIQUE;");
    let t = table(&c, "t");
    assert_eq!(t.primary_key.as_ref().unwrap().columns, vec!["id"]);
    assert_eq!(t.foreign_keys.len(), 1);
    assert_eq!(t.unique_constraints.len(), 1);
}

#[test]
fn serial_columns_are_not_null_with_default() {
    let c = pg("CREATE TABLE t (a serial, b bigserial, c smallserial, d serial4);");
    for col in ["a", "b", "c", "d"] {
        let def = table(&c, "t").get_column(col).unwrap();
        assert!(!def.nullable, "{col} should be NOT NULL");
        assert!(
            matches!(def.default, Some(DefaultValue::NextVal(_))),
            "{col} should have a nextval default: {:?}",
            def.default
        );
    }
    assert_eq!(column_type(&c, "t", "b"), SqlType::BigInt);
}

#[test]
fn identity_and_auto_increment_columns_are_not_null() {
    let c = pg("CREATE TABLE t (id int GENERATED ALWAYS AS IDENTITY, x int);");
    assert!(!nullable(&c, "t", "id"));
    let m = mysql("CREATE TABLE t (id INT AUTO_INCREMENT, x INT, PRIMARY KEY (id));");
    assert!(!nullable(&m, "t", "id"));
}

// =====================================================================
// 8. Warnings for skipped statements
// =====================================================================

#[track_caller]
fn single_warning(dialect: SqlDialect, schema: &str) -> Diagnostic {
    let (_, warnings) = build(dialect, schema);
    assert_eq!(warnings.len(), 1, "expected one warning: {warnings:#?}");
    let w = warnings.into_iter().next().unwrap();
    assert_eq!(w.severity, Severity::Warning);
    assert_eq!(w.kind, DiagnosticKind::ParseError);
    w
}

#[test]
fn unparseable_create_table_warns_with_name_and_location() {
    let schema = "CREATE TABLE p (id int);\n\n  CREATE TABLE child (x int) USING heap;\nCREATE TABLE after_it (y int);\n";
    let w = single_warning(SqlDialect::PostgreSQL, schema);
    assert!(w.message.contains("child"), "{}", w.message);
    assert!(w.message.contains("USING"), "{}", w.message);
    let span = w.span.expect("span");
    assert_eq!((span.line, span.column), (3, 3));
    assert_eq!(&schema[span.offset..span.offset + 12], "CREATE TABLE");

    // Surrounding statements are still processed
    let (c, _) = build(SqlDialect::PostgreSQL, schema);
    assert!(c.table_exists(&QualifiedName::new("p")));
    assert!(c.table_exists(&QualifiedName::new("after_it")));
}

#[test]
fn parser_location_in_warning_is_relative_to_input() {
    let schema = "CREATE TABLE p (id int);\nCREATE TABLE c (x int) TABLESPACE fast;";
    let w = single_warning(SqlDialect::PostgreSQL, schema);
    assert!(
        w.message.contains("Line: 2, Column: 24"),
        "parser location should be absolute: {}",
        w.message
    );
}

#[test]
fn warning_location_skips_leading_comments() {
    let w = single_warning(
        SqlDialect::PostgreSQL,
        "-- leading comment\nCREATE TABLE m (id int, k int);\n/* c */ CREATE TABLE m1 (x int COMPRESSION lz4);",
    );
    assert!(w.message.contains("m1"), "{}", w.message);
    let span = w.span.unwrap();
    assert_eq!((span.line, span.column), (3, 9));
}

#[test]
fn inherits_copies_parent_columns() {
    // sqlparser can't parse INHERITS; the table is built with its parents' columns
    let c = pg("CREATE TABLE p (id int NOT NULL, name text);
         CREATE TABLE q (extra2 int);
         CREATE TABLE c (extra int, name text) INHERITS (p, q);
         CREATE TABLE part (CONSTRAINT ck CHECK ((id > 0))) INHERITS (public.p);
         ALTER TABLE ONLY part ADD CONSTRAINT part_pkey PRIMARY KEY (id);");
    assert_eq!(
        table(&c, "c").column_names(),
        vec!["id", "name", "extra2", "extra"]
    );
    assert!(!nullable(&c, "c", "id"));
    assert_eq!(table(&c, "part").column_names(), vec!["id", "name"]);
    assert_clean(&c, "SELECT id, name, extra FROM c");
}

#[test]
fn inherits_missing_parent_warns() {
    let (c, warnings) = build(
        SqlDialect::PostgreSQL,
        "CREATE TABLE c (x int) INHERITS (missing);",
    );
    assert_eq!(table(&c, "c").column_names(), vec!["x"]);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].message.contains("missing"));
}

#[test]
fn partition_of_copies_parent_columns() {
    let c = pg(
        "CREATE TABLE m (id int NOT NULL, k int) PARTITION BY LIST (k);
         CREATE TABLE m1 PARTITION OF m FOR VALUES IN (1);
         CREATE TABLE IF NOT EXISTS public.m2 PARTITION OF public.m DEFAULT;",
    );
    assert_eq!(table(&c, "m1").column_names(), vec!["id", "k"]);
    assert_eq!(table(&c, "m2").column_names(), vec!["id", "k"]);
    assert_clean(&c, "SELECT id, k FROM m1");
}

#[test]
fn partition_of_missing_parent_warns() {
    let w = single_warning(
        SqlDialect::PostgreSQL,
        "CREATE TABLE m1 PARTITION OF missing FOR VALUES IN (1);",
    );
    assert!(w.message.contains("m1"), "{}", w.message);
}

#[test]
fn mysql_prefix_index_warns() {
    let w = single_warning(
        SqlDialect::MySQL,
        "CREATE TABLE `t` (`name` VARCHAR(100), KEY `idx` (`name`(50)));",
    );
    assert!(w.message.contains("t"), "{}", w.message);
}

#[test]
fn unparseable_view_and_alter_table_warn() {
    let (_, warnings) = build(
        SqlDialect::PostgreSQL,
        "CREATE TABLE t (a int);
         CREATE VIEW v AS SELECT FROM WHERE;
         ALTER TABLE t ADD COLUMN;",
    );
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
    assert!(warnings[0].message.contains('v'));
    assert!(warnings[1].message.contains('t'));
}

#[test]
fn unsupported_statements_stay_silent() {
    let (c, warnings) = build(
        SqlDialect::PostgreSQL,
        r#"
        CREATE EXTENSION IF NOT EXISTS "uuid-ossp" WITH SCHEMA public VERSION '1.1' CASCADE;
        CREATE OR REPLACE FUNCTION f() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$;
        CREATE TRIGGER trg BEFORE UPDATE ON t FOR EACH ROW EXECUTE FUNCTION f();
        CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS i ON t USING gin (a gin_trgm_ops) WITH (fastupdate = off);
        GRANT ALL ON ALL TABLES IN SCHEMA public TO app;
        SET search_path = public, pg_catalog;
        SELECT pg_catalog.set_config('search_path', '', false);
        COMMENT ON EXTENSION plpgsql IS 'PL/pgSQL';
        DO $$ BEGIN RAISE NOTICE 'x'; END $$;
        CREATE DOMAIN posint AS integer CHECK (VALUE > 0);
        ALTER TABLE t OWNER TO postgres;
        ALTER TABLE ONLY t REPLICA IDENTITY FULL;
        ALTER TABLE t ENABLE ROW LEVEL SECURITY;
        ALTER TABLE ONLY t ADD CONSTRAINT fk FOREIGN KEY (a) REFERENCES p;
        ALTER TABLE t ADD CONSTRAINT ex EXCLUDE USING gist (a WITH =);
        ALTER TYPE mood OWNER TO postgres;
        CREATE TABLE t (a int);
        "#,
    );
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert!(c.table_exists(&QualifiedName::new("t")));
}

// =====================================================================
// 9. Fallback statement splitter
// =====================================================================

/// An unparseable, silently skipped statement that forces statement-by-statement parsing
const FORCE_FALLBACK: &str = "DO $$ BEGIN PERFORM 1; END $$;\n";

#[test]
fn splitter_handles_semicolon_in_double_quoted_identifier() {
    let c = pg(&format!(
        "{FORCE_FALLBACK}CREATE TABLE \"a;b\" (\"x;y\" int, z int);\nCREATE TABLE next_one (id int);"
    ));
    assert_eq!(table(&c, "a;b").column_names(), vec!["x;y", "z"]);
    assert!(c.table_exists(&QualifiedName::new("next_one")));
}

#[test]
fn splitter_handles_postgres_strings_and_comments() {
    let c = pg(&format!(
        "{FORCE_FALLBACK}CREATE TABLE a (s text DEFAULT 'it''s; fine', t text DEFAULT E'esc\\'; still', u text DEFAULT 'back\\');
         -- a comment; with semicolon
         /* block; /* nested; */ still comment; */
         CREATE TABLE b (id int);"
    ));
    assert_eq!(table(&c, "a").column_names(), vec!["s", "t", "u"]);
    assert!(c.table_exists(&QualifiedName::new("b")));
}

#[test]
fn splitter_handles_dollar_quoted_bodies() {
    let c = pg(&format!(
        "{FORCE_FALLBACK}CREATE FUNCTION f() RETURNS int AS $body$ SELECT 1; SELECT 2; $body$ LANGUAGE sql;
         CREATE TABLE a (id int);"
    ));
    assert!(c.table_exists(&QualifiedName::new("a")));
}

#[test]
fn splitter_handles_mysql_backticks_and_backslash_escapes() {
    let c = mysql(
        "LOCK TABLES `x` WRITE, `y` READ NOWAIT FOR SOMETHING;
         CREATE TABLE `we;ird` (`a;b` INT, s VARCHAR(10) DEFAULT 'it\\';s', t VARCHAR(10) DEFAULT 'x\\\\');
         # hash comment; here
         CREATE TABLE after_it (id INT);",
    );
    assert_eq!(table(&c, "we;ird").column_names(), vec!["a;b", "s", "t"]);
    assert!(c.table_exists(&QualifiedName::new("after_it")));
}

// =====================================================================
// 10. Data type mappings
// =====================================================================

#[test]
fn float_types_map_by_precision() {
    let c = pg("CREATE TABLE t (a float, b float(10), c float(24), d float(25), e float(53));");
    assert_eq!(column_type(&c, "t", "a"), SqlType::DoublePrecision);
    assert_eq!(column_type(&c, "t", "b"), SqlType::Real);
    assert_eq!(column_type(&c, "t", "c"), SqlType::Real);
    assert_eq!(column_type(&c, "t", "d"), SqlType::DoublePrecision);
    assert_eq!(column_type(&c, "t", "e"), SqlType::DoublePrecision);
    assert_codes(
        &c,
        SqlDialect::PostgreSQL,
        "SELECT a FROM t WHERE a = 'abc'::text",
        &["E0003"],
    );
}

#[test]
fn mysql_text_and_blob_variants_map() {
    let c = mysql(
        "CREATE TABLE t (a TINYTEXT, b MEDIUMTEXT, c LONGTEXT, d TINYBLOB, e MEDIUMBLOB, f LONGBLOB, g FLOAT);",
    );
    for col in ["a", "b", "c"] {
        assert_eq!(column_type(&c, "t", col), SqlType::Text, "{col}");
    }
    for col in ["d", "e", "f"] {
        assert_eq!(column_type(&c, "t", col), SqlType::Bytea, "{col}");
    }
    assert_eq!(column_type(&c, "t", "g"), SqlType::DoublePrecision);
}

#[test]
fn bit_types_stay_unknown_and_unchecked() {
    let c = pg("CREATE TABLE t (a bit(3), b bit varying(5));");
    assert_eq!(column_type(&c, "t", "a"), SqlType::Unknown);
    assert_clean(&c, "SELECT a FROM t WHERE a = B'101' AND b = 'x'");
}

// =====================================================================
// Migration files: dbmate down sections and rollback file names
// =====================================================================

#[test]
fn dbmate_down_section_is_ignored() {
    let c = pg("-- migrate:up\n\
         CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL);\n\
         -- migrate:down\n\
         DROP TABLE users;\n");
    assert_clean(&c, "SELECT id, name FROM users;");
}

#[test]
fn dbmate_up_after_down_is_applied_again() {
    let c = pg("-- migrate:up transaction:false\n\
         CREATE TABLE a (id INT);\n\
         -- migrate:down\n\
         DROP TABLE a;\n\
         CREATE TABLE gone (id INT);\n\
         -- migrate:up\n\
         CREATE TABLE b (id INT);\n");
    assert_clean(&c, "SELECT a.id, b.id FROM a, b;");
    assert_codes(
        &c,
        SqlDialect::PostgreSQL,
        "SELECT id FROM gone",
        &["E0001"],
    );
}

#[test]
fn dbmate_down_section_in_unparseable_file_is_ignored() {
    // A statement sqlparser rejects forces the statement-by-statement fallback
    let c = pg("-- migrate:up\n\
         CREATE TABLE users (id INT);\n\
         CREATE NONSENSE STATEMENT;\n\
         -- migrate:down\n\
         DROP TABLE users;\n");
    assert_clean(&c, "SELECT id FROM users;");
}

#[test]
fn strip_down_migrations_keeps_offsets() {
    use sqlsift_core::schema::strip_down_migrations;
    let sql = "-- migrate:up\nCREATE TABLE t (id INT);\n-- migrate:down\nDROP TABLE t;\n";
    assert_eq!(
        strip_down_migrations(sql),
        "-- migrate:up\nCREATE TABLE t (id INT);\n               \n             \n"
    );
    // Files without markers are returned unchanged
    let plain = "CREATE TABLE t (id INT); -- migrate:downstream\n-- migrate:downgrade\n";
    assert_eq!(strip_down_migrations(plain), plain);
}

#[test]
fn rollback_migration_file_names() {
    use sqlsift_core::schema::is_rollback_migration;
    use std::path::Path;
    for name in [
        "000001_create_users.down.sql",
        "migrations/20240101_init.DOWN.sql",
        "U1__create_users.sql",
        "U2.1__add_column.sql",
        "U1_1__add_column.sql",
    ] {
        assert!(is_rollback_migration(Path::new(name)), "{name}");
    }
    for name in [
        "000001_create_users.up.sql",
        "V1__create_users.sql",
        "R__views.sql",
        "users.sql",
        "Users__x.sql",
        "U__nope.sql",
        "migration.sql",
        "download.sql",
        "down.sql/schema.sql",
    ] {
        assert!(!is_rollback_migration(Path::new(name)), "{name}");
    }
}
