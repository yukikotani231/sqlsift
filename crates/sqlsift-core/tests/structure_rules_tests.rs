//! Structural rules: duplicate names (E0009), duplicate target columns (E0010),
//! positions out of range (E0011) and misplaced aggregates (E0012).
//!
//! Every query reported here is rejected by the database (checked against
//! PostgreSQL 16 and SQLite 3.45); every query asserted clean is accepted.

use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::schema::{Catalog, SchemaBuilder};
use sqlsift_core::Analyzer;

const SCHEMA: &str = "
CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT, org_id INT);
CREATE TABLE orgs (id INT PRIMARY KEY, title TEXT);
CREATE SCHEMA auth;
CREATE TABLE auth.users (id INT PRIMARY KEY);
";

fn catalog(dialect: SqlDialect) -> Catalog {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(SCHEMA).expect("schema parses");
    builder.build().0
}

fn codes_in(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    let catalog = catalog(dialect);
    Analyzer::with_dialect(&catalog, dialect)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

fn codes(sql: &str) -> Vec<&'static str> {
    codes_in(SqlDialect::PostgreSQL, sql)
}

#[track_caller]
fn assert_reports(sql: &str, code: &str) {
    let found = codes(sql);
    assert!(
        found.contains(&code),
        "expected {code} for {sql}, got {found:?}"
    );
}

#[track_caller]
fn assert_clean(sql: &str) {
    assert_eq!(codes(sql), Vec::<&str>::new(), "{sql}");
}

// ---------------------------------------------------------------------
// E0009 duplicate-name
// ---------------------------------------------------------------------

#[test]
fn e0009_alias_given_twice() {
    assert_reports("SELECT u.id FROM users u JOIN orgs u ON true", "E0009");
    assert_reports("SELECT 1 FROM users, users", "E0009");
    assert_reports("SELECT 1 FROM users u, (SELECT 1) u", "E0009");
    assert_reports(
        "SELECT 1 FROM users JOIN (orgs JOIN users ON true) ON true",
        "E0009",
    );
}

#[test]
fn e0009_cte_name_given_twice() {
    assert_reports(
        "WITH x AS (SELECT 1), x AS (SELECT 2) SELECT * FROM x",
        "E0009",
    );
}

#[test]
fn e0009_distinct_names_are_fine() {
    assert_clean("SELECT a.id, b.id FROM users a JOIN users b ON a.id = b.org_id");
    // Two tables both named `users`, in different schemas
    assert_clean("SELECT 1 FROM auth.users CROSS JOIN public.users");
    // The same name in different query blocks
    assert_clean("SELECT u.id FROM users u WHERE EXISTS (SELECT 1 FROM orgs u WHERE u.id = 1)");
    assert_clean(
        "WITH x AS (SELECT 1 AS a) SELECT * FROM x, (WITH x AS (SELECT 2 AS b) SELECT * FROM x) y",
    );
}

#[test]
fn e0009_dialects() {
    // SQLite accepts a table alias given twice, but not a CTE name
    assert!(codes_in(SqlDialect::SQLite, "SELECT 1 FROM users CROSS JOIN users").is_empty());
    assert_eq!(
        codes_in(
            SqlDialect::SQLite,
            "WITH x AS (SELECT 1), x AS (SELECT 2) SELECT * FROM x"
        ),
        vec!["E0009"]
    );
    // MySQL aliases are case sensitive
    assert!(codes_in(SqlDialect::MySQL, "SELECT 1 FROM users U CROSS JOIN orgs u").is_empty());
    assert_eq!(
        codes_in(SqlDialect::MySQL, "SELECT 1 FROM users u CROSS JOIN orgs u"),
        vec!["E0009"]
    );
}

// ---------------------------------------------------------------------
// E0010 duplicate-target-column
// ---------------------------------------------------------------------

#[test]
fn e0010_insert_column_given_twice() {
    assert_eq!(
        codes("INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"),
        vec!["E0010"]
    );
    assert_eq!(
        codes_in(
            SqlDialect::MySQL,
            "INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"
        ),
        vec!["E0010"]
    );
    // SQLite accepts it
    assert!(codes_in(
        SqlDialect::SQLite,
        "INSERT INTO users (id, name, id) VALUES (1, 'a', 2)"
    )
    .is_empty());
}

#[test]
fn e0010_update_column_assigned_twice() {
    assert_eq!(
        codes("UPDATE users SET name = 'a', name = 'b'"),
        vec!["E0010"]
    );
    assert_eq!(
        codes(
            "INSERT INTO users (id, name) VALUES (1, 'a') \
             ON CONFLICT (id) DO UPDATE SET name = 'b', name = 'c'"
        ),
        vec!["E0010"]
    );
    // MySQL and SQLite apply the assignments in order
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        assert!(codes_in(dialect, "UPDATE users SET name = 'a', name = 'b'").is_empty());
    }
    assert_clean("UPDATE users SET name = 'a', email = 'b'");
}

// ---------------------------------------------------------------------
// E0011 position-out-of-range
// ---------------------------------------------------------------------

#[test]
fn e0011_order_and_group_by_positions() {
    assert_eq!(codes("SELECT id FROM users ORDER BY 2"), vec!["E0011"]);
    assert_eq!(codes("SELECT id FROM users ORDER BY 0"), vec!["E0011"]);
    assert_eq!(codes("SELECT id FROM users GROUP BY 3"), vec!["E0011"]);
    assert_eq!(
        codes("SELECT id FROM users UNION SELECT id FROM orgs ORDER BY 2"),
        vec!["E0011"]
    );
    assert_eq!(
        codes_in(SqlDialect::SQLite, "SELECT id FROM users ORDER BY 2"),
        vec!["E0011"]
    );
}

#[test]
fn e0011_positions_in_range_are_fine() {
    assert_clean("SELECT id, name FROM users ORDER BY 2, 1");
    assert_clean("SELECT * FROM users ORDER BY 4");
    assert_clean("SELECT org_id, count(*) FROM users GROUP BY 1 ORDER BY 2 DESC");
    assert_clean("SELECT id FROM users UNION SELECT id FROM orgs ORDER BY 1");
    // Constant expressions are not positions
    assert_clean("SELECT id FROM users ORDER BY 2 + 0");
    // Unknown columns: no count to compare with
    assert_clean("SELECT * FROM generate_series(1, 3) ORDER BY 2");
}

// ---------------------------------------------------------------------
// E0012 misplaced-aggregate
// ---------------------------------------------------------------------

#[test]
fn e0012_aggregates_where_not_allowed() {
    assert_eq!(
        codes("SELECT id FROM users WHERE count(*) > 1"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT 1 FROM users GROUP BY count(*)"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT 1 FROM users u JOIN orgs o ON max(o.id) = u.org_id"),
        vec!["E0012"]
    );
    assert_eq!(codes("UPDATE users SET org_id = max(id)"), vec!["E0012"]);
    assert_eq!(codes("DELETE FROM users WHERE sum(id) > 1"), vec!["E0012"]);
    assert_eq!(
        codes("INSERT INTO users (id, name) VALUES (count(*), 'a')"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_nested_aggregates() {
    assert_eq!(codes("SELECT max(count(*)) FROM users"), vec!["E0012"]);
    assert_eq!(
        codes("SELECT org_id FROM users GROUP BY org_id HAVING sum(count(*)) > 1"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_window_functions_where_not_allowed() {
    assert_eq!(
        codes("SELECT id FROM users WHERE row_number() OVER () > 1"),
        vec!["E0012"]
    );
    assert_eq!(
        codes("SELECT org_id FROM users GROUP BY org_id HAVING rank() OVER () = 1"),
        vec!["E0012"]
    );
}

#[test]
fn e0012_allowed_places_are_fine() {
    assert_clean("SELECT org_id, count(*) FROM users GROUP BY org_id HAVING count(*) > 1");
    assert_clean("SELECT org_id FROM users GROUP BY org_id ORDER BY count(*)");
    // Aggregates of a subquery belong to it
    assert_clean("SELECT id FROM users WHERE id > (SELECT max(id) FROM orgs)");
    assert_clean("SELECT id FROM users WHERE EXISTS (SELECT 1 FROM orgs HAVING count(*) > 0)");
    assert_clean("UPDATE users SET org_id = (SELECT max(id) FROM orgs)");
    // A window function over an aggregate
    assert_clean("SELECT org_id, sum(count(*)) OVER () FROM users GROUP BY org_id");
    assert_clean("SELECT id, row_number() OVER (ORDER BY id) FROM users");
    // A user-defined function named like an aggregate, schema-qualified
    assert_clean("SELECT id FROM users WHERE myschema.max(id) > 1");
}

#[test]
fn e0012_sqlite_scalar_min_max() {
    // With several arguments SQLite's min/max are scalar functions
    assert!(codes_in(
        SqlDialect::SQLite,
        "SELECT id FROM users WHERE max(id, 2) > 1"
    )
    .is_empty());
    assert_eq!(
        codes_in(SqlDialect::SQLite, "SELECT id FROM users WHERE max(id) > 1"),
        vec!["E0012"]
    );
}

// ---------------------------------------------------------------------
// E0013 generated-column-write
// ---------------------------------------------------------------------

const GENERATED: &str = "
CREATE TABLE g (
  id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  a INT,
  d INT GENERATED ALWAYS AS (a * 2) STORED NOT NULL,
  e INT GENERATED BY DEFAULT AS IDENTITY
);
";

fn generated_codes(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(GENERATED).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[test]
fn e0013_values_for_generated_columns() {
    let pg = |sql| generated_codes(SqlDialect::PostgreSQL, sql);
    assert_eq!(pg("INSERT INTO g (a, d) VALUES (1, 2)"), vec!["E0013"]);
    assert_eq!(pg("INSERT INTO g (d) SELECT 1"), vec!["E0013"]);
    assert_eq!(pg("INSERT INTO g VALUES (DEFAULT, 1, 5, 3)"), vec!["E0013"]);
    assert_eq!(pg("INSERT INTO g (id, a) VALUES (1, 1)"), vec!["E0013"]);
    assert_eq!(pg("UPDATE g SET d = 5"), vec!["E0013"]);
    assert_eq!(pg("UPDATE g SET id = 5"), vec!["E0013"]);
}

#[test]
fn e0013_default_and_by_default_identity_are_fine() {
    let pg = |sql| generated_codes(SqlDialect::PostgreSQL, sql);
    assert!(pg("INSERT INTO g (a) VALUES (1)").is_empty());
    assert!(pg("INSERT INTO g (a, d) VALUES (1, DEFAULT)").is_empty());
    assert!(pg("INSERT INTO g (id, a) VALUES (DEFAULT, 1)").is_empty());
    assert!(pg("INSERT INTO g VALUES (DEFAULT, 1, DEFAULT, 3)").is_empty());
    assert!(pg("INSERT INTO g (e, a) VALUES (1, 1)").is_empty());
    assert!(pg("UPDATE g SET d = DEFAULT, e = 5").is_empty());
}

#[test]
fn e0013_sqlite_positional_values_skip_generated_columns() {
    let schema =
        "CREATE TABLE s (id INTEGER, full TEXT GENERATED ALWAYS AS (id || 'x') STORED, z INT);";
    let mut builder = SchemaBuilder::with_dialect(SqlDialect::SQLite);
    builder.parse(schema).expect("schema parses");
    let catalog = builder.build().0;
    let codes = |sql| {
        Analyzer::with_dialect(&catalog, SqlDialect::SQLite)
            .analyze(sql)
            .iter()
            .map(sqlsift_core::Diagnostic::code)
            .collect::<Vec<_>>()
    };
    assert!(codes("INSERT INTO s VALUES (1, 2)").is_empty());
    assert_eq!(codes("INSERT INTO s VALUES (1, 2, 3)"), vec!["E0005"]);
    assert_eq!(
        codes("INSERT INTO s (id, full) VALUES (1, 'a')"),
        vec!["E0013"]
    );
    assert_eq!(codes("UPDATE s SET full = 'x'"), vec!["E0013"]);
}

// ---------------------------------------------------------------------
// E0014 unmatched-conflict-target
// ---------------------------------------------------------------------

fn conflict_codes(schema: &str, sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::new();
    builder.parse(schema).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::new(&catalog)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[test]
fn e0014_target_without_unique_key() {
    let schema = "CREATE TABLE t (id INT PRIMARY KEY, a INT, b INT, c TEXT, UNIQUE (a, b));
                  CREATE UNIQUE INDEX t_c ON t (c);";
    assert_eq!(
        conflict_codes(
            schema,
            "INSERT INTO t (id) VALUES (1) ON CONFLICT (a) DO NOTHING"
        ),
        vec!["E0014"]
    );
    for target in ["(id)", "(a, b)", "(b, a)", "(c)"] {
        let sql = format!("INSERT INTO t (id) VALUES (1) ON CONFLICT {target} DO NOTHING");
        assert!(conflict_codes(schema, &sql).is_empty(), "{sql}");
    }
    let schema = "CREATE TABLE t (x INT, y INT); ALTER TABLE t ADD CONSTRAINT t_y UNIQUE (y);";
    assert!(conflict_codes(
        schema,
        "INSERT INTO t (x) VALUES (1) ON CONFLICT (y) DO NOTHING"
    )
    .is_empty());
    assert_eq!(
        conflict_codes(
            schema,
            "INSERT INTO t (x) VALUES (1) ON CONFLICT (x) DO NOTHING"
        ),
        vec!["E0014"]
    );
}

#[test]
fn e0014_unknown_unique_keys_are_not_reported() {
    let sql = "INSERT INTO t (x) VALUES (1) ON CONFLICT (y) DO NOTHING";
    // Partial and expression unique indexes
    assert!(conflict_codes(
        "CREATE TABLE t (x INT, y INT); CREATE UNIQUE INDEX i ON t (x) WHERE y > 0;",
        sql
    )
    .is_empty());
    assert!(conflict_codes(
        "CREATE TABLE t (x INT, y INT); CREATE UNIQUE INDEX i ON t ((x + 1));",
        sql
    )
    .is_empty());
    // A partition gets the unique indexes of its parent
    assert!(conflict_codes(
        "CREATE TABLE p (x INT, y INT) PARTITION BY RANGE (x);
         CREATE TABLE t PARTITION OF p FOR VALUES FROM (0) TO (10);",
        sql
    )
    .is_empty());
}

// ---------------------------------------------------------------------
// E0005 column-count-mismatch: column aliases, subqueries, rows
// ---------------------------------------------------------------------

#[test]
fn e0005_column_alias_lists() {
    assert_eq!(codes("SELECT * FROM orgs AS o(a, b, c)"), vec!["E0005"]);
    assert_eq!(
        codes("WITH x(a, b) AS (SELECT 1) SELECT a FROM x"),
        vec!["E0005"]
    );
    assert_eq!(
        codes("SELECT * FROM (SELECT 1, 2) s(a, b, c)"),
        vec!["E0005"]
    );
    // PostgreSQL renames the first columns only
    assert_clean("SELECT * FROM orgs AS o(a)");
    assert_clean("WITH x(a) AS (SELECT 1, 2) SELECT a FROM x");
    // MySQL and SQLite need one name per column of a CTE
    assert_eq!(
        codes_in(
            SqlDialect::SQLite,
            "WITH x(a) AS (SELECT 1, 2) SELECT a FROM x"
        ),
        vec!["E0005"]
    );
}

#[test]
fn e0005_subquery_and_row_shapes() {
    assert_eq!(
        codes("SELECT id FROM users WHERE id IN (SELECT id, title FROM orgs)"),
        vec!["E0005"]
    );
    assert_eq!(
        codes("SELECT id FROM users WHERE id = (SELECT id, title FROM orgs)"),
        vec!["E0005"]
    );
    assert_eq!(
        codes("SELECT (SELECT id, title FROM orgs LIMIT 1) FROM users"),
        vec!["E0005"]
    );
    assert_eq!(
        codes("SELECT id FROM users WHERE (id, name) IN (SELECT id FROM orgs)"),
        vec!["E0005"]
    );
    assert_eq!(
        codes("SELECT id FROM users WHERE (id, name) = (1, 'a', 2)"),
        vec!["E0005"]
    );
    assert_clean("SELECT id FROM users WHERE (id, name) = (SELECT id, title FROM orgs)");
    assert_clean("SELECT id FROM users WHERE (id, name) IN (SELECT id, title FROM orgs)");
    assert_clean("SELECT id FROM users WHERE EXISTS (SELECT id, title FROM orgs)");
    assert_clean("UPDATE users SET (name, email) = (SELECT title, title FROM orgs LIMIT 1)");
}

// ---------------------------------------------------------------------
// E0015 distinct-order-by
// ---------------------------------------------------------------------

#[test]
fn e0015_distinct_ordered_by_unselected_column() {
    assert_eq!(
        codes("SELECT DISTINCT name FROM users ORDER BY email"),
        vec!["E0015"]
    );
    assert_eq!(
        codes("SELECT DISTINCT ON (name) id FROM users ORDER BY email"),
        vec!["E0015"]
    );
    assert_eq!(
        codes("SELECT DISTINCT ON (name, org_id) id FROM users ORDER BY org_id, id"),
        vec!["E0015"]
    );
    // SQLite accepts it
    assert!(codes_in(
        SqlDialect::SQLite,
        "SELECT DISTINCT name FROM users ORDER BY email"
    )
    .is_empty());
}

#[test]
fn e0015_selected_columns_are_fine() {
    assert_clean("SELECT DISTINCT name FROM users ORDER BY name");
    assert_clean("SELECT DISTINCT u.name FROM users u ORDER BY name");
    assert_clean("SELECT DISTINCT name AS n FROM users ORDER BY n");
    assert_clean("SELECT DISTINCT * FROM users ORDER BY email");
    assert_clean("SELECT DISTINCT name FROM users ORDER BY 1");
    assert_clean("SELECT DISTINCT ON (name) id FROM users ORDER BY name, email");
    assert_clean("SELECT DISTINCT ON (name, org_id) id FROM users ORDER BY org_id, name, id");
    assert_clean("SELECT DISTINCT ON (name, org_id) id FROM users ORDER BY org_id");
}

// ---------------------------------------------------------------------
// E0016 grouping-error
// ---------------------------------------------------------------------

const GROUPING: &str = "
CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL, email TEXT, org_id INT);
CREATE TABLE orgs (id INT PRIMARY KEY, title TEXT);
CREATE TABLE pairs (a INT, b INT, v INT, PRIMARY KEY (a, b));
";

fn grouping_codes(dialect: SqlDialect, sql: &str) -> Vec<&'static str> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    builder.parse(GROUPING).expect("schema parses");
    let catalog = builder.build().0;
    Analyzer::with_dialect(&catalog, dialect)
        .analyze(sql)
        .iter()
        .map(sqlsift_core::Diagnostic::code)
        .collect()
}

#[test]
fn e0016_ungrouped_columns() {
    let pg = |sql| grouping_codes(SqlDialect::PostgreSQL, sql);
    for sql in [
        "SELECT name, count(*) FROM users",
        "SELECT email FROM users GROUP BY name",
        "SELECT u.name, o.title FROM users u JOIN orgs o ON o.id = u.org_id GROUP BY u.id",
        "SELECT org_id FROM users GROUP BY org_id ORDER BY name",
        "SELECT org_id FROM users GROUP BY org_id HAVING name = 'a'",
        "SELECT name FROM users HAVING count(*) > 1",
        "SELECT a, v FROM pairs GROUP BY a",
        "SELECT CASE WHEN org_id > 1 THEN name END FROM users GROUP BY org_id",
        "SELECT x.n, x.m FROM (SELECT name AS n, email AS m FROM users) x GROUP BY x.n",
    ] {
        assert_eq!(pg(sql), vec!["E0016"], "{sql}");
    }
}

#[test]
fn e0016_grouped_or_aggregated_columns_are_fine() {
    let pg = |sql| grouping_codes(SqlDialect::PostgreSQL, sql);
    for sql in [
        // Functional dependency on the primary key
        "SELECT id, name FROM users GROUP BY id",
        "SELECT u.id, u.name FROM users u GROUP BY u.id",
        "SELECT u.name, o.title FROM users u JOIN orgs o ON o.id = u.org_id GROUP BY u.id, o.id",
        "SELECT a, b, v FROM pairs GROUP BY a, b",
        // Grouped expressions, positions and output aliases
        "SELECT lower(name), count(*) FROM users GROUP BY lower(name)",
        "SELECT lower(name), count(*) FROM users GROUP BY lower(users.name)",
        "SELECT name || '!' FROM users GROUP BY name",
        "SELECT org_id FROM users GROUP BY 1",
        "SELECT org_id AS o, count(*) FROM users GROUP BY o",
        "SELECT org_id, count(*) AS c FROM users GROUP BY org_id ORDER BY c",
        "SELECT users.name FROM users GROUP BY name",
        // Aggregates, subqueries and window functions
        "SELECT count(*) FROM users HAVING count(*) > 1",
        "SELECT org_id, max(name), string_agg(email, ',') FROM users GROUP BY org_id",
        "SELECT org_id, count(*) FILTER (WHERE name = 'a') FROM users GROUP BY org_id",
        "SELECT org_id, (SELECT max(title) FROM orgs WHERE orgs.id = users.org_id) FROM users GROUP BY org_id",
        "SELECT org_id, row_number() OVER (ORDER BY org_id) FROM users GROUP BY org_id",
        "SELECT 1 FROM users GROUP BY org_id ORDER BY count(*)",
        // A function that may be a user-defined aggregate
        "SELECT org_id, my_median(id) FROM users GROUP BY org_id",
        // No aggregation
        "SELECT name FROM users",
    ] {
        assert!(pg(sql).is_empty(), "{sql}: {:?}", pg(sql));
    }
}

#[test]
fn e0016_other_dialects_and_unknown_shapes() {
    // MySQL depends on ONLY_FULL_GROUP_BY; SQLite allows bare columns
    for dialect in [SqlDialect::MySQL, SqlDialect::SQLite] {
        assert!(grouping_codes(dialect, "SELECT name, count(*) FROM users").is_empty());
    }
    let pg = |sql| grouping_codes(SqlDialect::PostgreSQL, sql);
    assert!(pg("SELECT name, count(*) FROM users GROUP BY ROLLUP (org_id)").is_empty());
    assert!(pg("SELECT u.name FROM users u JOIN orgs o USING (id) GROUP BY id").is_empty());
    assert!(pg("SELECT x, count(*) FROM generate_series(1, 3) GROUP BY 1").is_empty());
}
