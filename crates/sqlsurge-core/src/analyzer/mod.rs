//! SQL analyzer module

mod resolver;

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::Catalog;

pub use resolver::NameResolver;

/// SQL Analyzer - validates SQL against a schema catalog
pub struct Analyzer<'a> {
    catalog: &'a Catalog,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Analyzer<'a> {
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
        }
    }

    /// Analyze a SQL query and return diagnostics
    pub fn analyze(&mut self, sql: &str) -> Vec<Diagnostic> {
        self.diagnostics.clear();

        // Parse the SQL
        let dialect = PostgreSqlDialect {};
        let statements = match Parser::parse_sql(&dialect, sql) {
            Ok(stmts) => stmts,
            Err(e) => {
                self.diagnostics.push(
                    Diagnostic::error(DiagnosticKind::ParseError, format!("Parse error: {}", e))
                        .with_span(Span::new(0, sql.len().min(50))),
                );
                return std::mem::take(&mut self.diagnostics);
            }
        };

        // Analyze each statement
        for stmt in &statements {
            let mut resolver = NameResolver::new(self.catalog);
            resolver.resolve_statement(stmt);
            self.diagnostics.extend(resolver.into_diagnostics());
        }

        std::mem::take(&mut self.diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaBuilder;

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
        let diagnostics =
            analyzer.analyze("INSERT INTO users (id, name) VALUES (1, 'test', 'extra')");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::ColumnCountMismatch);
    }

    #[test]
    fn test_insert_column_count_mismatch_fewer_values() {
        let catalog = setup_catalog();
        let mut analyzer = Analyzer::new(&catalog);

        // 3 columns but 2 values
        let diagnostics =
            analyzer.analyze("INSERT INTO users (id, name, email) VALUES (1, 'test')");
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
        let diagnostics = analyzer.analyze(
            "WITH active_users AS (SELECT id FROM users) SELECT id, name FROM active_users",
        );
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("products"))
            .unwrap();
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("employees"))
            .unwrap();
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("accounts"))
            .unwrap();
        let id_col = table.get_column("id").unwrap();
        assert!(!id_col.nullable, "IDENTITY column should be NOT NULL");
        assert!(
            matches!(id_col.identity, Some(crate::schema::IdentityKind::Always)),
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("logs"))
            .unwrap();
        let id_col = table.get_column("id").unwrap();
        assert!(!id_col.nullable, "IDENTITY column should be NOT NULL");
        assert!(
            matches!(
                id_col.identity,
                Some(crate::schema::IdentityKind::ByDefault)
            ),
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
            .get_view(&crate::schema::QualifiedName::new("active_users"))
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

        let view = catalog
            .get_view(&crate::schema::QualifiedName::new("user_info"))
            .unwrap();
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("users"))
            .unwrap();
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("users"))
            .unwrap();
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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("users"))
            .unwrap();
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

        assert!(catalog.table_exists(&crate::schema::QualifiedName::new("people")));
        assert!(!catalog.table_exists(&crate::schema::QualifiedName::new("users")));

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

        let table = catalog
            .get_table(&crate::schema::QualifiedName::new("orders"))
            .unwrap();
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
}
