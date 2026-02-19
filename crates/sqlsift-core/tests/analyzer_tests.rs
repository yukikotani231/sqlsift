// Integration tests for SQL analyzer
use sqlsift_core::analyzer::Analyzer;
use sqlsift_core::dialect::SqlDialect;
use sqlsift_core::error::DiagnosticKind;
use sqlsift_core::schema::{Catalog, IdentityKind, QualifiedName, SchemaBuilder};
use sqlsift_core::types::SqlType;

fn setup_catalog() -> Catalog {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT
            );

            CREATE TABLE orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL,
                total DECIMAL(10, 2)
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    catalog
}

#[test]
fn test_valid_select() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("SELECT id, name FROM users");
    assert!(
        diagnostics.is_empty(),
        "Expected no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_table_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("SELECT * FROM nonexistent");
    // Table not found error should be first
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("SELECT nonexistent_column FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_column_not_found_qualified() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Column with table qualifier that doesn't exist
    let diagnostics = analyzer.analyze("SELECT u.nonexistent FROM users u");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("nonexistent"));
}

#[test]
fn test_table_alias_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Using alias that wasn't defined
    let diagnostics = analyzer.analyze("SELECT x.id FROM users u");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
    assert!(diagnostics[0].message.contains("'x'"));
}

#[test]
fn test_ambiguous_column() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Both users and orders have 'id' column
    let diagnostics =
        analyzer.analyze("SELECT id FROM users JOIN orders ON users.id = orders.user_id");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::AmbiguousColumn);
    assert!(diagnostics[0].message.contains("ambiguous"));
}

#[test]
fn test_ambiguous_column_resolved_with_qualifier() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Ambiguity resolved by qualifying with table name
    let diagnostics =
        analyzer.analyze("SELECT users.id FROM users JOIN orders ON users.id = orders.user_id");
    assert!(
        diagnostics.is_empty(),
        "Expected no errors when column is qualified: {:?}",
        diagnostics
    );
}

#[test]
fn test_parse_error() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Invalid SQL syntax
    let diagnostics = analyzer.analyze("SELECT FROM WHERE");
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ParseError);
}

#[test]
fn test_join_condition_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // JOIN condition references non-existent column
    let diagnostics =
        analyzer.analyze("SELECT u.id FROM users u JOIN orders o ON o.customer_id = u.id");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("customer_id"));
}

#[test]
fn test_valid_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Valid JOIN with correct column names
    let diagnostics = analyzer
        .analyze("SELECT u.id, u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id");
    assert!(
        diagnostics.is_empty(),
        "Expected no errors for valid JOIN: {:?}",
        diagnostics
    );
}

#[test]
fn test_error_has_span() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("SELECT bad_column FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].span.is_some(),
        "Diagnostic should have span information"
    );
    let span = diagnostics[0].span.unwrap();
    assert!(span.line > 0, "Span should have line number");
    assert!(span.column > 0, "Span should have column number");
}

// ========== INSERT Tests ==========

#[test]
fn test_insert_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics =
        analyzer.analyze("INSERT INTO users (id, name, email) VALUES (1, 'test', 'a@b.com')");
    assert!(
        diagnostics.is_empty(),
        "Valid INSERT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_insert_table_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("INSERT INTO nonexistent (id) VALUES (1)");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_insert_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("INSERT INTO users (id, username) VALUES (1, 'test')");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("username"));
}

#[test]
fn test_insert_column_count_mismatch() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // 2 columns but 3 values
    let diagnostics = analyzer.analyze("INSERT INTO users (id, name) VALUES (1, 'test', 'extra')");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnCountMismatch);
}

#[test]
fn test_insert_column_count_mismatch_fewer_values() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // 3 columns but 2 values
    let diagnostics = analyzer.analyze("INSERT INTO users (id, name, email) VALUES (1, 'test')");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnCountMismatch);
}

// ========== UPDATE Tests ==========

#[test]
fn test_update_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("UPDATE users SET name = 'new' WHERE id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid UPDATE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_update_table_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("UPDATE nonexistent SET name = 'new'");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_update_set_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("UPDATE users SET username = 'new' WHERE id = 1");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("username"));
}

#[test]
fn test_update_where_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("UPDATE users SET name = 'new' WHERE user_id = 1");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("user_id"));
}

// ========== DELETE Tests ==========

#[test]
fn test_delete_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("DELETE FROM users WHERE id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid DELETE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_delete_table_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("DELETE FROM nonexistent WHERE id = 1");
    assert!(!diagnostics.is_empty());
    // First error should be table not found
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_delete_where_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("DELETE FROM users WHERE user_id = 1");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("user_id"));
}

// ========== Subquery Tests ==========

#[test]
fn test_subquery_in_where_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Subquery referencing its own table
    let diagnostics =
        analyzer.analyze("SELECT id FROM users WHERE id IN (SELECT user_id FROM orders)");
    assert!(
        diagnostics.is_empty(),
        "Valid subquery should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_correlated_subquery_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Correlated subquery referencing outer query's table
    let diagnostics = analyzer.analyze(
            "SELECT u.id, u.name FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id)",
        );
    assert!(
        diagnostics.is_empty(),
        "Valid correlated subquery should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_subquery_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Subquery with invalid column
    let diagnostics =
        analyzer.analyze("SELECT id FROM users WHERE id IN (SELECT nonexistent FROM orders)");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("nonexistent"));
}

#[test]
fn test_scalar_subquery_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Scalar subquery in SELECT
    let diagnostics = analyzer.analyze(
        "SELECT id, (SELECT COUNT(*) FROM orders WHERE orders.user_id = users.id) FROM users",
    );
    assert!(
        diagnostics.is_empty(),
        "Valid scalar subquery should have no errors: {:?}",
        diagnostics
    );
}

// ========== CTE Tests ==========

#[test]
fn test_cte_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze(
        "WITH active_users AS (SELECT id, name FROM users) SELECT id, name FROM active_users",
    );
    assert!(
        diagnostics.is_empty(),
        "Valid CTE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_cte_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CTE with column that doesn't exist in the CTE definition
    let diagnostics = analyzer
        .analyze("WITH active_users AS (SELECT id FROM users) SELECT id, name FROM active_users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("name"));
}

// ========== CHECK Constraint Tests ==========

#[test]
fn test_check_constraint_table_level() {
    let schema_sql = r#"
            CREATE TABLE products (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                price DECIMAL(10, 2) NOT NULL,
                CONSTRAINT price_positive CHECK (price > 0)
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("products")).unwrap();
    assert_eq!(table.check_constraints.len(), 1);
    assert_eq!(
        table.check_constraints[0].name.as_deref(),
        Some("price_positive")
    );
    assert!(table.check_constraints[0].expression.contains("price"));

    // Queries against the table should still work
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name, price FROM products");
    assert!(
        diagnostics.is_empty(),
        "Valid query on table with CHECK constraint should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_check_constraint_column_level() {
    let schema_sql = r#"
            CREATE TABLE employees (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                age INTEGER CHECK (age >= 18)
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("employees")).unwrap();
    assert_eq!(table.check_constraints.len(), 1);
    assert!(table.check_constraints[0].expression.contains("age"));
}

// ========== ENUM Type Tests ==========

#[test]
fn test_enum_type_definition() {
    let schema_sql = r#"
            CREATE TYPE status AS ENUM ('active', 'inactive', 'pending');

            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                status status NOT NULL
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    // Verify enum was parsed
    let enum_def = catalog.get_enum("status").unwrap();
    assert_eq!(enum_def.values, vec!["active", "inactive", "pending"]);

    // Queries against table with enum column should work
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name, status FROM users");
    assert!(
        diagnostics.is_empty(),
        "Valid query on table with enum column should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_enum_type_exists() {
    let schema_sql = r#"
            CREATE TYPE priority AS ENUM ('low', 'medium', 'high', 'critical');
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    assert!(catalog.enum_exists("priority"));
    assert!(!catalog.enum_exists("nonexistent"));

    let enum_def = catalog.get_enum("priority").unwrap();
    assert_eq!(enum_def.values.len(), 4);
}

// ========== IDENTITY Column Tests ==========

#[test]
fn test_identity_column_always() {
    let schema_sql = r#"
            CREATE TABLE accounts (
                id INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                name TEXT NOT NULL
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("accounts")).unwrap();
    let id_col = table.get_column("id").unwrap();
    assert!(!id_col.nullable, "IDENTITY column should be NOT NULL");
    assert!(
        matches!(id_col.identity, Some(IdentityKind::Always)),
        "Expected GENERATED ALWAYS AS IDENTITY"
    );
}

#[test]
fn test_identity_column_by_default() {
    let schema_sql = r#"
            CREATE TABLE logs (
                id BIGINT GENERATED BY DEFAULT AS IDENTITY,
                message TEXT NOT NULL
            );
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("logs")).unwrap();
    let id_col = table.get_column("id").unwrap();
    assert!(!id_col.nullable, "IDENTITY column should be NOT NULL");
    assert!(
        matches!(id_col.identity, Some(IdentityKind::ByDefault)),
        "Expected GENERATED BY DEFAULT AS IDENTITY"
    );

    // Queries should work normally
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, message FROM logs");
    assert!(
        diagnostics.is_empty(),
        "Valid query on table with IDENTITY column should have no errors: {:?}",
        diagnostics
    );
}

// ========== VIEW Tests ==========

#[test]
fn test_view_definition_and_query() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT,
                active BOOLEAN DEFAULT true
            );

            CREATE VIEW active_users AS
                SELECT id, name, email FROM users WHERE active = true;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    // Verify view was parsed
    let view = catalog
        .get_view(&QualifiedName::new("active_users"))
        .unwrap();
    assert_eq!(view.columns, vec!["id", "name", "email"]);
    assert!(!view.materialized);

    // Query against view should work
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name FROM active_users");
    assert!(
        diagnostics.is_empty(),
        "Valid query on VIEW should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_view_column_not_found() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT
            );

            CREATE VIEW user_names AS
                SELECT id, name FROM users;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    // Query with column not in view should error
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT email FROM user_names");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("email"));
}

#[test]
fn test_view_with_alias() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT
            );

            CREATE VIEW user_emails AS
                SELECT id, name, email FROM users;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    // Query view with alias
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT ue.id, ue.name FROM user_emails ue");
    assert!(
        diagnostics.is_empty(),
        "Valid query on VIEW with alias should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_view_with_explicit_columns() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT
            );

            CREATE VIEW user_info (user_id, user_name) AS
                SELECT id, name FROM users;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let view = catalog.get_view(&QualifiedName::new("user_info")).unwrap();
    assert_eq!(view.columns, vec!["user_id", "user_name"]);

    // Query with explicit view column names
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT user_id, user_name FROM user_info");
    assert!(
        diagnostics.is_empty(),
        "Query with explicit view columns should have no errors: {:?}",
        diagnostics
    );

    // Original column name should not work
    let diagnostics = analyzer.analyze("SELECT id FROM user_info");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_view_join_with_table() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL
            );

            CREATE TABLE orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL,
                total DECIMAL(10, 2)
            );

            CREATE VIEW user_orders AS
                SELECT u.id AS user_id, u.name, o.total
                FROM users u JOIN orders o ON o.user_id = u.id;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT user_id, name, total FROM user_orders");
    assert!(
        diagnostics.is_empty(),
        "Query on VIEW with JOIN should have no errors: {:?}",
        diagnostics
    );
}

// ========== ALTER TABLE Tests ==========

#[test]
fn test_alter_table_add_column() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL
            );

            ALTER TABLE users ADD COLUMN email TEXT;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("users")).unwrap();
    assert_eq!(table.columns.len(), 3);
    assert!(table.get_column("email").is_some());

    // Query with new column should work
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name, email FROM users");
    assert!(
        diagnostics.is_empty(),
        "Query with ALTER TABLE added column should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_alter_table_drop_column() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT,
                obsolete TEXT
            );

            ALTER TABLE users DROP COLUMN obsolete;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("users")).unwrap();
    assert_eq!(table.columns.len(), 3);
    assert!(table.get_column("obsolete").is_none());

    // Query with dropped column should error
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT obsolete FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_alter_table_rename_column() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT
            );

            ALTER TABLE users RENAME COLUMN email TO email_address;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("users")).unwrap();
    assert!(table.get_column("email").is_none());
    assert!(table.get_column("email_address").is_some());

    // Query with renamed column should work
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name, email_address FROM users");
    assert!(
        diagnostics.is_empty(),
        "Renamed column query should work: {:?}",
        diagnostics
    );

    // Old column name should error
    let diagnostics = analyzer.analyze("SELECT email FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_alter_table_rename_table() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL
            );

            ALTER TABLE users RENAME TO people;
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    assert!(catalog.table_exists(&QualifiedName::new("people")));
    assert!(!catalog.table_exists(&QualifiedName::new("users")));

    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT id, name FROM people");
    assert!(
        diagnostics.is_empty(),
        "Query on renamed table should work: {:?}",
        diagnostics
    );

    let diagnostics = analyzer.analyze("SELECT id, name FROM users");
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_alter_table_add_constraint() {
    let schema_sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL
            );

            CREATE TABLE orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL
            );

            ALTER TABLE orders ADD CONSTRAINT fk_user
                FOREIGN KEY (user_id) REFERENCES users(id);
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("orders")).unwrap();
    assert_eq!(table.foreign_keys.len(), 1);
    assert_eq!(table.foreign_keys[0].name.as_deref(), Some("fk_user"));
}

#[test]
fn test_alter_table_nonexistent_warns() {
    let schema_sql = r#"
            ALTER TABLE nonexistent ADD COLUMN foo TEXT;
        "#;

    let mut builder = SchemaBuilder::new();
    // parse returns Ok because warnings don't cause failure
    builder.parse(schema_sql).unwrap();
    let (_, diagnostics) = builder.build();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TableNotFound),
        "Should warn about nonexistent table: {:?}",
        diagnostics
    );
}

#[test]
fn test_cte_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Reference to undefined CTE
    let diagnostics = analyzer.analyze("SELECT id FROM undefined_cte");
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

// ========== Derived Table (Subquery in FROM) Tests ==========

#[test]
fn test_derived_table_valid() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics =
        analyzer.analyze("SELECT sub.id, sub.name FROM (SELECT id, name FROM users) AS sub");
    assert!(
        diagnostics.is_empty(),
        "Derived table query should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_derived_table_column_not_found() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics =
        analyzer.analyze("SELECT sub.nonexistent FROM (SELECT id, name FROM users) AS sub");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_derived_table_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze(
            "SELECT u.name, sub.order_id FROM users u JOIN (SELECT id AS order_id FROM orders) AS sub ON u.id = sub.order_id",
        );
    assert!(
        diagnostics.is_empty(),
        "Derived table in JOIN should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_derived_table_with_alias_expression() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer
        .analyze("SELECT sub.user_count FROM (SELECT COUNT(*) AS user_count FROM users) AS sub");
    assert!(
        diagnostics.is_empty(),
        "Derived table with aliased expression should work: {:?}",
        diagnostics
    );
}

// ========== MySQL Dialect Tests ==========

fn setup_mysql_catalog() -> Catalog {
    let schema_sql = r#"
            CREATE TABLE users (
                id INT AUTO_INCREMENT PRIMARY KEY,
                username VARCHAR(50) NOT NULL,
                email VARCHAR(255) NOT NULL,
                age TINYINT UNSIGNED,
                status ENUM('active', 'inactive', 'banned') DEFAULT 'active',
                login_count MEDIUMINT UNSIGNED DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE KEY uk_email (email)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

            CREATE TABLE posts (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                user_id INT NOT NULL,
                title VARCHAR(200) NOT NULL,
                body MEDIUMTEXT,
                view_count INT UNSIGNED DEFAULT 0,
                is_published TINYINT(1) DEFAULT 0,
                published_at DATETIME,
                FOREIGN KEY (user_id) REFERENCES users(id)
            ) ENGINE=InnoDB;
        "#;

    let mut builder = SchemaBuilder::with_dialect(SqlDialect::MySQL);
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    catalog
}

#[test]
fn test_mysql_schema_parsing() {
    let catalog = setup_mysql_catalog();

    let table = catalog.get_table(&QualifiedName::new("users")).unwrap();

    // AUTO_INCREMENT column should be NOT NULL
    let id_col = table.get_column("id").unwrap();
    assert!(!id_col.nullable, "AUTO_INCREMENT column should be NOT NULL");
    assert!(id_col.is_primary_key);

    // TINYINT column
    let age_col = table.get_column("age").unwrap();
    assert_eq!(age_col.data_type, SqlType::TinyInt);

    // ENUM column
    let status_col = table.get_column("status").unwrap();
    assert!(
        matches!(&status_col.data_type, SqlType::Custom(name) if name == "ENUM"),
        "ENUM column should be Custom(\"ENUM\"): {:?}",
        status_col.data_type
    );

    // MEDIUMINT column
    let count_col = table.get_column("login_count").unwrap();
    assert_eq!(count_col.data_type, SqlType::MediumInt);
}

#[test]
fn test_mysql_valid_select() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze("SELECT id, username, email, age, status FROM users");
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL SELECT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_join() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze(
            "SELECT p.title, u.username FROM posts p INNER JOIN users u ON p.user_id = u.id WHERE p.is_published = 1",
        );
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL JOIN should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_insert() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze(
            "INSERT INTO users (username, email, age, status) VALUES ('test', 'test@example.com', 25, 'active')",
        );
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL INSERT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_column_not_found() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze("SELECT usrname FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("usrname"));
}

#[test]
fn test_mysql_table_not_found() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze("SELECT * FROM nonexistent");
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
}

#[test]
fn test_mysql_subquery() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze(
        "SELECT username FROM users WHERE id IN (SELECT user_id FROM posts WHERE is_published = 1)",
    );
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL subquery should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_cte() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze(
            "WITH active_users AS (SELECT id, username FROM users WHERE status = 'active') SELECT au.username FROM active_users au",
        );
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL CTE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_update() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze("UPDATE posts SET is_published = 1 WHERE id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL UPDATE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_mysql_delete() {
    let catalog = setup_mysql_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);

    let diagnostics = analyzer.analyze("DELETE FROM posts WHERE user_id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid MySQL DELETE should have no errors: {:?}",
        diagnostics
    );
}

// ========== Complex Query Pattern Tests ==========

#[test]
fn test_deeply_nested_subquery() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // 3-level nested subquery with all columns explicitly qualified
    let diagnostics = analyzer.analyze(
        "SELECT users.id FROM users WHERE users.id IN (
                SELECT orders.user_id FROM orders WHERE orders.total > (
                    SELECT AVG(o2.total) FROM orders o2 WHERE o2.user_id IN (
                        SELECT u2.id FROM users u2 WHERE u2.name LIKE 'A%'
                    )
                )
            )",
    );
    assert!(
        diagnostics.is_empty(),
        "Deeply nested subquery should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_multiple_ctes_with_dependencies() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Multiple CTEs where later ones reference earlier ones
    let diagnostics = analyzer.analyze(
            "WITH
                active_users AS (SELECT id, name FROM users),
                user_orders AS (SELECT user_id, total FROM orders WHERE user_id IN (SELECT active_users.id FROM active_users)),
                summary AS (SELECT user_id, COUNT(*) AS order_count FROM user_orders GROUP BY user_id)
            SELECT au.name, s.order_count
            FROM active_users au
            JOIN summary s ON au.id = s.user_id",
        );
    assert!(
        diagnostics.is_empty(),
        "Multiple dependent CTEs should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_multiple_ctes_invalid_reference() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CTE references another CTE that doesn't exist
    let diagnostics = analyzer.analyze(
        "WITH
                users_cte AS (SELECT id FROM users),
                orders_cte AS (SELECT user_id FROM nonexistent_cte)
            SELECT * FROM orders_cte",
    );
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TableNotFound);
    assert!(diagnostics[0].message.contains("nonexistent_cte"));
}

#[test]
fn test_large_join_four_tables() {
    // Create extended schema
    let extended_schema = r#"
            CREATE TABLE users (id SERIAL PRIMARY KEY, name VARCHAR(100));
            CREATE TABLE orders (id SERIAL PRIMARY KEY, user_id INTEGER);
            CREATE TABLE products (id SERIAL PRIMARY KEY, name TEXT);
            CREATE TABLE order_items (order_id INTEGER, product_id INTEGER, quantity INTEGER);
        "#;

    let mut builder = SchemaBuilder::new();
    builder.parse(extended_schema).unwrap();
    let (catalog, _) = builder.build();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze(
        "SELECT u.name, o.id, p.name, oi.quantity
            FROM users u
            JOIN orders o ON u.id = o.user_id
            JOIN order_items oi ON o.id = oi.order_id
            JOIN products p ON oi.product_id = p.id",
    );
    assert!(
        diagnostics.is_empty(),
        "4-table JOIN should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_error_message_suggestion_typo() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Typo in column name should provide suggestion
    let diagnostics = analyzer.analyze("SELECT naem FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    // Check if suggestion is provided
    assert!(
        diagnostics[0].help.is_some(),
        "Should provide typo suggestion"
    );
    if let Some(ref help) = diagnostics[0].help {
        assert!(help.contains("name"), "Should suggest 'name': {}", help);
    }
}

#[test]
fn test_error_message_suggestion_table_typo() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Typo in table name
    let diagnostics = analyzer.analyze("SELECT * FROM userz");
    assert!(!diagnostics.is_empty(), "Should have at least one error");
    // First error should be TableNotFound
    let table_error = diagnostics
        .iter()
        .find(|d| d.kind == DiagnosticKind::TableNotFound);
    assert!(table_error.is_some(), "Should have TableNotFound error");
    // TableNotFound error always has help text
    let table_error = table_error.unwrap();
    assert!(
        table_error.help.is_some(),
        "TableNotFound should have help text"
    );
    // The error message should mention the typo'd table name
    assert!(
        table_error.message.contains("userz"),
        "Error message should mention 'userz': {}",
        table_error.message
    );
}

#[test]
fn test_subquery_scope_isolation() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Non-LATERAL subquery should not see outer FROM tables
    let diagnostics = analyzer.analyze(
        "SELECT u.id
            FROM users u
            WHERE EXISTS (SELECT 1 FROM orders WHERE user_id = u.id)",
    );
    assert!(
        diagnostics.is_empty(),
        "Correlated subquery should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_derived_table_scope_isolation() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Non-LATERAL derived table cannot reference outer tables
    let diagnostics = analyzer.analyze(
        "SELECT u.id, sub.total
            FROM users u,
                (SELECT user_id, SUM(total) AS total FROM orders GROUP BY user_id) sub
            WHERE u.id = sub.user_id",
    );
    assert!(
        diagnostics.is_empty(),
        "Derived table with proper reference should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_ambiguous_column_in_complex_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Both tables have 'id' column - should be ambiguous without qualifier
    let diagnostics = analyzer.analyze("SELECT id FROM users u JOIN orders o ON u.id = o.user_id");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::AmbiguousColumn);
    assert!(diagnostics[0].message.contains("id"));
    assert!(diagnostics[0].message.contains("ambiguous"));
}

#[test]
fn test_union_column_count_validation() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // UNION with different column counts
    // Note: This is currently not validated (limitation)
    // This test documents current behavior
    let diagnostics = analyzer.analyze(
        "SELECT id, name FROM users
            UNION
            SELECT id FROM orders",
    );
    // Current implementation doesn't validate UNION column count
    // This is a known limitation - just document that the query doesn't crash
    let _ = diagnostics;
}

#[test]
fn test_self_join_with_aliases() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze(
        "SELECT u1.name AS manager, u2.name AS employee
            FROM users u1
            JOIN users u2 ON u1.id = u2.id",
    );
    assert!(
        diagnostics.is_empty(),
        "Self-join should work with aliases: {:?}",
        diagnostics
    );
}

#[test]
fn test_cross_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let diagnostics = analyzer.analyze("SELECT u.name, o.id FROM users u CROSS JOIN orders o");
    assert!(
        diagnostics.is_empty(),
        "CROSS JOIN should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_natural_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // NATURAL JOIN automatically joins on columns with the same name
    let diagnostics = analyzer.analyze("SELECT u.name FROM users u NATURAL JOIN orders");
    // Current implementation should handle NATURAL JOIN
    // Even if 'id' exists in both tables, NATURAL JOIN is a valid construct
    assert!(
        diagnostics.is_empty() || diagnostics[0].kind != DiagnosticKind::ParseError,
        "NATURAL JOIN should be parseable: {:?}",
        diagnostics
    );
}

// ==================== Issue #14: UNNEST WITH ORDINALITY ====================

#[test]
fn test_unnest_with_ordinality_column_alias() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // unnest() WITH ORDINALITY AS t(id, row_number) should resolve alias columns
    let diagnostics = analyzer.analyze(
        "SELECT id, row_number FROM unnest(ARRAY[1,2,3]) WITH ORDINALITY AS t(id, row_number)",
    );
    assert!(
        diagnostics.is_empty(),
        "unnest WITH ORDINALITY columns should be resolvable: {:?}",
        diagnostics
    );
}

#[test]
fn test_unnest_with_ordinality_qualified_columns() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Qualified references: t.id, t.row_number
    let diagnostics = analyzer.analyze(
        "SELECT t.id, t.row_number FROM unnest(ARRAY[1,2,3]) WITH ORDINALITY AS t(id, row_number)",
    );
    assert!(
        diagnostics.is_empty(),
        "Qualified unnest columns should be resolvable: {:?}",
        diagnostics
    );
}

#[test]
fn test_unnest_with_ordinality_in_cte() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Issue #14 exact reproduction case (simplified without $2::uuid[])
    let diagnostics = analyzer.analyze(
        "WITH all_ids AS (
            SELECT id, row_number FROM unnest(ARRAY[1,2,3]) WITH ORDINALITY AS t(id, row_number)
        )
        SELECT * FROM all_ids",
    );
    assert!(
        diagnostics.is_empty(),
        "unnest WITH ORDINALITY in CTE should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_unnest_with_ordinality_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Join unnest result with a real table
    let diagnostics = analyzer.analyze(
        "SELECT u.name, t.id
         FROM users u
         JOIN unnest(ARRAY[1,2,3]) WITH ORDINALITY AS t(id, row_number)
           ON u.id = t.id",
    );
    assert!(
        diagnostics.is_empty(),
        "unnest WITH ORDINALITY in JOIN should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_unnest_without_alias_columns() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // unnest without explicit column aliases - should not crash
    let diagnostics = analyzer.analyze("SELECT * FROM unnest(ARRAY[1,2,3]) WITH ORDINALITY AS t");
    // No column names defined, so * won't have specific columns to validate
    // Just verify it doesn't crash or produce parse errors
    for d in &diagnostics {
        assert_ne!(
            d.kind,
            DiagnosticKind::ParseError,
            "Should not produce parse errors: {:?}",
            d
        );
    }
}

// ==================== Issue #15: UUID string literal compatibility ====================

fn setup_uuid_catalog() -> Catalog {
    let schema_sql = r#"
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            name VARCHAR(256)
        );
    "#;
    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    catalog
}

#[test]
fn test_uuid_string_literal_comparison() {
    let catalog = setup_uuid_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // UUID column compared with UUID-formatted string literal should NOT error
    let diagnostics =
        analyzer.analyze("SELECT * FROM users WHERE id = '123e4567-e89b-12d3-a456-426614174000'");
    assert!(
        diagnostics.is_empty(),
        "UUID column compared with string literal should not error: {:?}",
        diagnostics
    );
}

#[test]
fn test_uuid_string_literal_in_join() {
    let schema_sql = r#"
        CREATE TABLE users (id UUID PRIMARY KEY, name VARCHAR(256));
        CREATE TABLE sessions (id SERIAL, user_id UUID);
    "#;
    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    let mut analyzer = Analyzer::new(&catalog);

    // JOIN on UUID columns should work
    let diagnostics =
        analyzer.analyze("SELECT u.name FROM users u JOIN sessions s ON u.id = s.user_id");
    assert!(
        diagnostics.is_empty(),
        "JOIN on UUID columns should work: {:?}",
        diagnostics
    );
}

#[test]
fn test_uuid_integer_mismatch() {
    let catalog = setup_uuid_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // UUID compared with integer should still error
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = 42");
    assert!(
        !diagnostics.is_empty(),
        "UUID compared with integer should produce type mismatch"
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
}

// ========== INSERT/UPDATE Type Checking Tests ==========

#[test]
fn test_insert_type_mismatch() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // id is INTEGER (SERIAL), inserting a string should be a type mismatch
    let diagnostics = analyzer.analyze("INSERT INTO users (id) VALUES ('text')");
    assert_eq!(
        diagnostics.len(),
        1,
        "Should detect type mismatch in INSERT VALUES: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
    assert!(diagnostics[0].message.contains("id"));
}

#[test]
fn test_insert_type_compatible() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // id is INTEGER, inserting a number should be fine
    let diagnostics = analyzer.analyze("INSERT INTO users (id) VALUES (42)");
    assert!(
        diagnostics.is_empty(),
        "Compatible INSERT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_insert_null_compatible() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // NULL should be compatible with any column type
    let diagnostics = analyzer.analyze("INSERT INTO users (id) VALUES (NULL)");
    assert!(
        diagnostics.is_empty(),
        "NULL INSERT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_update_type_mismatch() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // id is INTEGER, setting to a string should be a type mismatch
    let diagnostics = analyzer.analyze("UPDATE users SET id = 'text'");
    assert_eq!(
        diagnostics.len(),
        1,
        "Should detect type mismatch in UPDATE SET: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
    assert!(diagnostics[0].message.contains("id"));
}

#[test]
fn test_update_type_compatible() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // name is VARCHAR, setting to a string should be fine
    let diagnostics = analyzer.analyze("UPDATE users SET name = 'new_name'");
    assert!(
        diagnostics.is_empty(),
        "Compatible UPDATE should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_update_multiple_type_errors() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // id is INTEGER, name is VARCHAR - both set to wrong types
    let diagnostics = analyzer.analyze("UPDATE orders SET user_id = 'text', total = true");
    assert_eq!(
        diagnostics.len(),
        2,
        "Should detect multiple type mismatches in UPDATE: {:?}",
        diagnostics
    );
    assert!(diagnostics
        .iter()
        .all(|d| d.kind == DiagnosticKind::TypeMismatch));
}

// ========== CAST Type Inference Tests ==========

#[test]
fn test_cast_type_inference_no_false_positive() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CAST(name AS INTEGER) should be inferred as INTEGER, compatible with INTEGER column
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = CAST(name AS INTEGER)");
    assert!(
        diagnostics.is_empty(),
        "CAST to INTEGER should be compatible with INTEGER column: {:?}",
        diagnostics
    );
}

#[test]
fn test_cast_type_inference_detects_mismatch() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CAST(id AS TEXT) produces TEXT, compared with INTEGER column => type mismatch
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = CAST(id AS TEXT)");
    assert_eq!(
        diagnostics.len(),
        1,
        "Should detect type mismatch: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
}

#[test]
fn test_cast_in_insert_compatible() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CAST('123' AS INTEGER) should be compatible with INTEGER column
    let diagnostics =
        analyzer.analyze("INSERT INTO orders (user_id) VALUES (CAST('123' AS INTEGER))");
    assert!(
        diagnostics.is_empty(),
        "CAST to INTEGER should be compatible with INTEGER column in INSERT: {:?}",
        diagnostics
    );
}

#[test]
fn test_cast_in_arithmetic() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // CAST(name AS INTEGER) + 1 should be numeric, compatible with INTEGER column
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = CAST(name AS INTEGER) + 1");
    assert!(
        diagnostics.is_empty(),
        "CAST to INTEGER in arithmetic should be compatible: {:?}",
        diagnostics
    );
}

// ========== SQLite Dialect Tests ==========

fn setup_sqlite_catalog() -> Catalog {
    let schema_sql = r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT,
            age INTEGER
        );

        CREATE TABLE posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            title TEXT NOT NULL,
            body TEXT,
            FOREIGN KEY (user_id) REFERENCES users(id)
        );
    "#;

    let mut builder = SchemaBuilder::with_dialect(SqlDialect::SQLite);
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    catalog
}

// ========== Function Return Type Inference ==========

#[test]
fn test_function_count_returns_bigint() {
    // COUNT() returns BIGINT, comparing with TEXT should error
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE name = COUNT(*)");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TypeMismatch),
        "COUNT() (bigint) compared with TEXT column should produce type mismatch: {:?}",
        diagnostics
    );
}

#[test]
fn test_function_count_compatible_with_integer() {
    // COUNT() returns BIGINT, comparing with INTEGER should be fine (numeric compatibility)
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = COUNT(*)");
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.kind == DiagnosticKind::TypeMismatch)
        .collect();
    assert!(
        type_errors.is_empty(),
        "COUNT() (bigint) compared with INTEGER should be compatible: {:?}",
        type_errors
    );
}

#[test]
fn test_function_sum_returns_numeric() {
    // SUM(integer_col) returns BIGINT, should not match TEXT
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM orders WHERE SUM(user_id) = 'text'");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TypeMismatch),
        "SUM() compared with TEXT should produce type mismatch: {:?}",
        diagnostics
    );
}

#[test]
fn test_function_upper_returns_text() {
    // UPPER() returns TEXT, comparing with INTEGER should error
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE id = UPPER(name)");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TypeMismatch),
        "UPPER() (text) compared with INTEGER should produce type mismatch: {:?}",
        diagnostics
    );
}

#[test]
fn test_function_upper_compatible_with_text() {
    // UPPER() returns TEXT, comparing with TEXT should be fine
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE name = UPPER(email)");
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.kind == DiagnosticKind::TypeMismatch)
        .collect();
    assert!(
        type_errors.is_empty(),
        "UPPER() (text) compared with TEXT should be compatible: {:?}",
        type_errors
    );
}

#[test]
fn test_function_length_returns_integer() {
    // LENGTH() returns INTEGER, comparing with TEXT should error
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE name = LENGTH(email)");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TypeMismatch),
        "LENGTH() (integer) compared with TEXT should produce type mismatch: {:?}",
        diagnostics
    );
}

#[test]
fn test_function_coalesce_inherits_arg_type() {
    // COALESCE(integer_col, 0) should return INTEGER
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics = analyzer.analyze("SELECT * FROM users WHERE name = COALESCE(id, 0)");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TypeMismatch),
        "COALESCE(integer) compared with TEXT should produce type mismatch: {:?}",
        diagnostics
    );
}

#[test]
fn test_function_in_insert_type_check() {
    // INSERT with function result type mismatch
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let diagnostics =
        analyzer.analyze("INSERT INTO users (id, name) VALUES (LENGTH('test'), 'alice')");
    let type_errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.kind == DiagnosticKind::TypeMismatch)
        .collect();
    assert!(
        type_errors.is_empty(),
        "LENGTH() returns INTEGER, compatible with INTEGER column: {:?}",
        type_errors
    );
}

#[test]
fn test_sqlite_schema_parsing() {
    let catalog = setup_sqlite_catalog();

    assert!(
        catalog.table_exists(&QualifiedName::new("users")),
        "users table should exist"
    );
    assert!(
        catalog.table_exists(&QualifiedName::new("posts")),
        "posts table should exist"
    );

    let users = catalog.get_table(&QualifiedName::new("users")).unwrap();
    assert_eq!(users.columns.len(), 4);

    let id_col = users.get_column("id").unwrap();
    assert!(id_col.is_primary_key);
    assert!(!id_col.nullable);
}

#[test]
fn test_sqlite_valid_select() {
    let catalog = setup_sqlite_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::SQLite);

    let diagnostics = analyzer.analyze("SELECT id, name, email FROM users");
    assert!(
        diagnostics.is_empty(),
        "Valid SQLite SELECT should have no errors: {:?}",
        diagnostics
    );
}

#[test]
fn test_sqlite_autoincrement() {
    let schema_sql = r#"
        CREATE TABLE counters (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            value INTEGER NOT NULL
        );
    "#;

    let mut builder = SchemaBuilder::with_dialect(SqlDialect::SQLite);
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();

    let table = catalog.get_table(&QualifiedName::new("counters")).unwrap();
    let id_col = table.get_column("id").unwrap();
    assert!(id_col.is_primary_key, "AUTOINCREMENT column should be PK");
    assert!(!id_col.nullable, "AUTOINCREMENT column should be NOT NULL");
}

#[test]
fn test_sqlite_insert() {
    let catalog = setup_sqlite_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::SQLite);

    // Valid INSERT
    let diagnostics =
        analyzer.analyze("INSERT INTO users (name, email, age) VALUES ('Alice', 'a@b.com', 30)");
    assert!(
        diagnostics.is_empty(),
        "Valid SQLite INSERT should have no errors: {:?}",
        diagnostics
    );

    // Type mismatch: age is INTEGER, inserting TEXT
    let diagnostics =
        analyzer.analyze("INSERT INTO users (name, age) VALUES ('Alice', 'not_a_number')");
    assert_eq!(
        diagnostics.len(),
        1,
        "SQLite INSERT type mismatch should be detected: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
}

#[test]
fn test_sqlite_update() {
    let catalog = setup_sqlite_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::SQLite);

    // Valid UPDATE
    let diagnostics = analyzer.analyze("UPDATE users SET name = 'Bob' WHERE id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid SQLite UPDATE should have no errors: {:?}",
        diagnostics
    );

    // Type mismatch: age is INTEGER, setting to TEXT
    let diagnostics = analyzer.analyze("UPDATE users SET age = 'old'");
    assert_eq!(
        diagnostics.len(),
        1,
        "SQLite UPDATE type mismatch should be detected: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
}

#[test]
fn test_sqlite_delete() {
    let catalog = setup_sqlite_catalog();
    let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::SQLite);

    let diagnostics = analyzer.analyze("DELETE FROM users WHERE id = 1");
    assert!(
        diagnostics.is_empty(),
        "Valid SQLite DELETE should have no errors: {:?}",
        diagnostics
    );

    // Column not found
    let diagnostics = analyzer.analyze("DELETE FROM users WHERE nonexistent = 1");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

// ============================================================
// Inline disable directive tests
// ============================================================

#[test]
fn test_inline_disable_same_line() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // Without directive: should report error
    let diagnostics = analyzer.analyze("SELECT bad_col FROM users");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);

    // With directive: should suppress
    let diagnostics = analyzer.analyze("SELECT bad_col FROM users -- sqlsift:disable E0002");
    assert!(
        diagnostics.is_empty(),
        "Inline disable should suppress E0002: {:?}",
        diagnostics
    );
}

#[test]
fn test_inline_disable_next_line() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let sql = "-- sqlsift:disable E0001\nSELECT * FROM nonexistent";
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Standalone disable should suppress next line: {:?}",
        diagnostics
    );
}

#[test]
fn test_inline_disable_multiple_codes() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let sql = "-- sqlsift:disable E0001, E0002\nSELECT bad_col FROM nonexistent";
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Should suppress both E0001 and E0002: {:?}",
        diagnostics
    );
}

#[test]
fn test_inline_disable_all() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let sql = "SELECT bad_col FROM nonexistent -- sqlsift:disable";
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Disable all should suppress everything: {:?}",
        diagnostics
    );
}

#[test]
fn test_inline_disable_only_affects_specified_line() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let sql = "-- sqlsift:disable E0002\nSELECT bad_col FROM users;\nSELECT another_bad FROM users";
    let diagnostics = analyzer.analyze(sql);
    // Line 2 should be suppressed, line 3 should still report
    assert_eq!(
        diagnostics.len(),
        1,
        "Only line 2 should be suppressed: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
    assert!(diagnostics[0].message.contains("another_bad"));
}

#[test]
fn test_inline_disable_wrong_code_not_suppressed() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    // E0001 directive should not suppress E0002
    let sql = "SELECT bad_col FROM users -- sqlsift:disable E0001";
    let diagnostics = analyzer.analyze(sql);
    assert_eq!(
        diagnostics.len(),
        1,
        "E0001 directive should not suppress E0002: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

#[test]
fn test_inline_disable_in_string_not_treated_as_directive() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);

    let sql = "SELECT '-- sqlsift:disable E0002', bad_col FROM users";
    let diagnostics = analyzer.analyze(sql);
    assert_eq!(
        diagnostics.len(),
        1,
        "Directive inside string should be ignored: {:?}",
        diagnostics
    );
    assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnNotFound);
}

// ============================================================
// Issue #56: INSERT ... RETURNING columns in CTEs
// ============================================================

#[test]
fn test_cte_insert_returning_columns() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let sql = r#"
        WITH new_user AS (
            INSERT INTO users (name, email)
            VALUES ('Alice', 'alice@example.com')
            RETURNING id, name, email
        )
        SELECT new_user.id, new_user.name, new_user.email FROM new_user
    "#;
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "INSERT RETURNING columns should be accessible in subsequent queries: {:?}",
        diagnostics
    );
}

#[test]
fn test_cte_insert_returning_used_in_join() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let sql = r#"
        WITH new_user AS (
            INSERT INTO users (name, email)
            VALUES ('Bob', 'bob@example.com')
            RETURNING id, name
        ),
        user_posts AS (
            SELECT nu.id, nu.name
            FROM new_user nu
        )
        SELECT up.id, up.name FROM user_posts up
    "#;
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "INSERT RETURNING columns should be usable in subsequent CTEs: {:?}",
        diagnostics
    );
}

// ============================================================
// Issue #57: Subquery scope isolation
// ============================================================

#[test]
fn test_subquery_in_update_no_false_ambiguity() {
    let schema_sql = r#"
        CREATE TABLE purchases (
            id UUID PRIMARY KEY,
            latest_purchase_ref_id UUID
        );
        CREATE TABLE latest_purchase_refs (
            id UUID PRIMARY KEY,
            latest_purchase_id UUID
        );
    "#;
    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    let mut analyzer = Analyzer::new(&catalog);
    let sql = r#"
        UPDATE purchases
        SET latest_purchase_ref_id = NULL
        WHERE latest_purchase_ref_id IN (
            SELECT id FROM latest_purchase_refs
            WHERE latest_purchase_id = '00000000-0000-0000-0000-000000000000'
        )
    "#;
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Subquery columns should not be ambiguous with outer table: {:?}",
        diagnostics
    );
}

#[test]
fn test_subquery_in_delete_no_false_ambiguity() {
    let schema_sql = r#"
        CREATE TABLE orders (id SERIAL PRIMARY KEY, status TEXT);
        CREATE TABLE archived_orders (id SERIAL PRIMARY KEY, reason TEXT);
    "#;
    let mut builder = SchemaBuilder::new();
    builder.parse(schema_sql).unwrap();
    let (catalog, _) = builder.build();
    let mut analyzer = Analyzer::new(&catalog);
    let sql = r#"
        DELETE FROM orders
        WHERE id IN (SELECT id FROM archived_orders)
    "#;
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "Subquery in DELETE should have isolated scope: {:?}",
        diagnostics
    );
}

#[test]
fn test_exists_subquery_no_false_ambiguity() {
    let catalog = setup_catalog();
    let mut analyzer = Analyzer::new(&catalog);
    let sql = r#"
        SELECT u.id FROM users u
        WHERE EXISTS (SELECT id FROM orders WHERE user_id = 1)
    "#;
    let diagnostics = analyzer.analyze(sql);
    assert!(
        diagnostics.is_empty(),
        "EXISTS subquery should have isolated scope: {:?}",
        diagnostics
    );
}
