//! Schema builder - converts SQL AST to Catalog

use sqlparser::ast::{
    AlterTableOperation, ColumnOption, ColumnOptionDef, ObjectName, Statement, TableConstraint,
    UserDefinedTypeRepresentation,
};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind};
use crate::schema::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, IdentityKind,
    PrimaryKeyDef, QualifiedName, TableDef, UniqueConstraintDef, ViewDef,
};
use crate::types::SqlType;

/// Builder for constructing a Catalog from SQL schema definitions
pub struct SchemaBuilder {
    catalog: Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
}

impl SchemaBuilder {
    pub fn new() -> Self {
        Self {
            catalog: Catalog::new(),
            diagnostics: Vec::new(),
            dialect: SqlDialect::default(),
        }
    }

    pub fn with_dialect(dialect: SqlDialect) -> Self {
        Self {
            catalog: Catalog::new(),
            diagnostics: Vec::new(),
            dialect,
        }
    }

    /// Parse SQL schema definitions and build the catalog
    pub fn parse(&mut self, sql: &str) -> Result<(), Vec<Diagnostic>> {
        let dialect = self.dialect.parser_dialect();

        // Try parsing the entire SQL first (fast path)
        match Parser::parse_sql(dialect.as_ref(), sql) {
            Ok(statements) => {
                for stmt in statements {
                    self.process_statement(&stmt);
                }
            }
            Err(_) => {
                // Fall back to statement-by-statement parsing to skip unsupported syntax
                self.parse_statements_individually(sql);
            }
        }

        if self
            .diagnostics
            .iter()
            .any(|d| d.severity == crate::error::Severity::Error)
        {
            Err(std::mem::take(&mut self.diagnostics))
        } else {
            Ok(())
        }
    }

    /// Parse SQL statements individually, skipping those that fail to parse.
    /// This allows sqlsift to handle schema files containing unsupported syntax
    /// (e.g., CREATE FUNCTION, CREATE TRIGGER, CREATE DOMAIN) by gracefully
    /// skipping unparseable statements while still processing the rest.
    fn parse_statements_individually(&mut self, sql: &str) {
        let dialect = self.dialect.parser_dialect();

        for raw_stmt in split_sql_statements(sql) {
            let trimmed = raw_stmt.trim();
            if trimmed.is_empty() {
                continue;
            }

            match Parser::parse_sql(dialect.as_ref(), trimmed) {
                Ok(stmts) => {
                    for stmt in stmts {
                        self.process_statement(&stmt);
                    }
                }
                Err(_) => {
                    // Silently skip unparseable statements (functions, triggers, etc.)
                }
            }
        }
    }

    /// Process a single SQL statement
    fn process_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::CreateTable(create) => {
                self.process_create_table(create);
            }
            Statement::CreateType {
                name,
                representation,
            } => {
                self.process_create_type(name, representation);
            }
            Statement::CreateView {
                name,
                columns,
                query,
                materialized,
                ..
            } => {
                self.process_create_view(name, columns, query, *materialized);
            }
            Statement::AlterTable {
                name, operations, ..
            } => {
                self.process_alter_table(name, operations);
            }
            _ => {}
        }
    }

    /// Process CREATE TABLE statement
    fn process_create_table(&mut self, create: &sqlparser::ast::CreateTable) {
        let name = object_name_to_qualified(&create.name);
        let mut table = TableDef::new(name);

        // Process columns
        for column in &create.columns {
            let col_name = column.name.value.clone();
            let data_type = SqlType::from_ast(&column.data_type);

            let mut col_def = ColumnDef::new(&col_name, data_type);

            // Process column options
            for option in &column.options {
                self.process_column_option(&mut col_def, &mut table, option);
            }

            table.columns.insert(col_name, col_def);
        }

        // Process table constraints
        for constraint in &create.constraints {
            self.process_table_constraint(&mut table, constraint);
        }

        self.catalog.add_table(table);
    }

    /// Process CREATE VIEW statement
    fn process_create_view(
        &mut self,
        name: &ObjectName,
        columns: &[sqlparser::ast::ViewColumnDef],
        query: &sqlparser::ast::Query,
        materialized: bool,
    ) {
        let qualified = object_name_to_qualified(name);

        // Determine column names: explicit column list or inferred from SELECT
        let column_names = if !columns.is_empty() {
            columns.iter().map(|c| c.name.value.clone()).collect()
        } else {
            self.infer_view_columns(&query.body)
        };

        let view = ViewDef {
            name: qualified,
            columns: column_names,
            materialized,
        };
        self.catalog.add_view(view);
    }

    /// Infer column names from a SELECT body for VIEW definition
    fn infer_view_columns(&self, set_expr: &sqlparser::ast::SetExpr) -> Vec<String> {
        use sqlparser::ast::{Expr, SelectItem, SetExpr};

        let mut columns = Vec::new();

        if let SetExpr::Select(select) = set_expr {
            for item in &select.projection {
                match item {
                    SelectItem::UnnamedExpr(Expr::Identifier(ident)) => {
                        columns.push(ident.value.clone());
                    }
                    SelectItem::ExprWithAlias { alias, .. } => {
                        columns.push(alias.value.clone());
                    }
                    SelectItem::UnnamedExpr(Expr::CompoundIdentifier(idents)) => {
                        if let Some(col) = idents.last() {
                            columns.push(col.value.clone());
                        }
                    }
                    SelectItem::Wildcard(_) => {
                        // Expand * by looking up FROM tables in the catalog
                        for table_with_joins in &select.from {
                            self.expand_wildcard_columns(&table_with_joins.relation, &mut columns);
                        }
                    }
                    SelectItem::QualifiedWildcard(name, _) => {
                        // table.* - try to expand from the specified table
                        let table_name = object_name_to_qualified(name);
                        if let Some(table_def) = self.catalog.get_table(&table_name) {
                            for col_name in table_def.columns.keys() {
                                columns.push(col_name.clone());
                            }
                        }
                    }
                    _ => {
                        // Other expressions without alias - generate placeholder
                        columns.push(format!("?column?{}", columns.len() + 1));
                    }
                }
            }
        }

        columns
    }

    /// Expand wildcard columns from a table factor
    fn expand_wildcard_columns(
        &self,
        factor: &sqlparser::ast::TableFactor,
        columns: &mut Vec<String>,
    ) {
        use sqlparser::ast::TableFactor;
        if let TableFactor::Table { name, .. } = factor {
            let table_name = object_name_to_qualified(name);
            if let Some(table_def) = self.catalog.get_table(&table_name) {
                for col_name in table_def.columns.keys() {
                    columns.push(col_name.clone());
                }
            } else if let Some(view_def) = self.catalog.get_view(&table_name) {
                for col_name in &view_def.columns {
                    columns.push(col_name.clone());
                }
            }
        }
    }

    /// Process ALTER TABLE statement
    fn process_alter_table(&mut self, name: &ObjectName, operations: &[AlterTableOperation]) {
        // Skip ALTER TABLE if it contains no schema-affecting operations.
        // Operations like OWNER TO, ENABLE/DISABLE TRIGGER, etc. don't affect
        // the schema catalog and should not produce warnings.
        let has_schema_operations = operations.iter().any(|op| {
            matches!(
                op,
                AlterTableOperation::AddColumn { .. }
                    | AlterTableOperation::DropColumn { .. }
                    | AlterTableOperation::RenameColumn { .. }
                    | AlterTableOperation::RenameTable { .. }
                    | AlterTableOperation::AddConstraint(_)
            )
        });

        if !has_schema_operations {
            return;
        }

        let table_name = object_name_to_qualified(name);

        // Check if table exists
        if !self.catalog.table_exists(&table_name) {
            self.diagnostics.push(
                Diagnostic::warning(
                    DiagnosticKind::TableNotFound,
                    format!(
                        "ALTER TABLE references table '{}' which was not found in schema",
                        table_name
                    ),
                )
                .with_help("Ensure the CREATE TABLE statement appears before ALTER TABLE"),
            );
            return;
        }

        for operation in operations {
            match operation {
                AlterTableOperation::AddColumn { column_def, .. } => {
                    let col_name = column_def.name.value.clone();
                    let data_type = SqlType::from_ast(&column_def.data_type);
                    let mut col = ColumnDef::new(&col_name, data_type);

                    // Process column options
                    // We need a temporary mutable table reference for check constraints
                    // Process non-table options first
                    for option in &column_def.options {
                        match &option.option {
                            ColumnOption::Null => col.nullable = true,
                            ColumnOption::NotNull => col.nullable = false,
                            ColumnOption::Default(expr) => {
                                col.default = Some(expr_to_default(expr));
                            }
                            ColumnOption::Unique { is_primary, .. } => {
                                if *is_primary {
                                    col.is_primary_key = true;
                                    col.nullable = false;
                                }
                            }
                            ColumnOption::Generated {
                                generated_as,
                                generation_expr,
                                ..
                            } => {
                                if generation_expr.is_none() {
                                    use sqlparser::ast::GeneratedAs;
                                    let kind = match generated_as {
                                        GeneratedAs::Always => IdentityKind::Always,
                                        GeneratedAs::ByDefault => IdentityKind::ByDefault,
                                        _ => continue,
                                    };
                                    col.identity = Some(kind);
                                    col.nullable = false;
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        // Collect check constraints from column options
                        for option in &column_def.options {
                            if let ColumnOption::Check(expr) = &option.option {
                                let check = CheckConstraintDef {
                                    name: option.name.as_ref().map(|n| n.value.clone()),
                                    expression: expr.to_string(),
                                };
                                table.check_constraints.push(check);
                            }
                        }
                        table.columns.insert(col_name, col);
                    }
                }
                AlterTableOperation::DropColumn { column_name, .. } => {
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        table.columns.shift_remove(&column_name.value);
                    }
                }
                AlterTableOperation::RenameColumn {
                    old_column_name,
                    new_column_name,
                } => {
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        if let Some(mut col) = table.columns.shift_remove(&old_column_name.value) {
                            col.name = new_column_name.value.clone();
                            table.columns.insert(new_column_name.value.clone(), col);
                        }
                    }
                }
                AlterTableOperation::RenameTable {
                    table_name: new_name,
                } => {
                    let new_qualified = object_name_to_qualified(new_name);
                    let schema_name = table_name
                        .schema
                        .as_ref()
                        .unwrap_or(&self.catalog.default_schema)
                        .clone();
                    if let Some(schema) = self.catalog.schemas.get_mut(&schema_name) {
                        if let Some(mut table) = schema.tables.shift_remove(&table_name.name) {
                            table.name = new_qualified.clone();
                            schema.tables.insert(new_qualified.name, table);
                        }
                    }
                }
                AlterTableOperation::AddConstraint(constraint) => {
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        // Reuse the same constraint processing logic
                        match constraint {
                            TableConstraint::PrimaryKey { columns, name, .. } => {
                                let pk = crate::schema::PrimaryKeyDef {
                                    name: name.as_ref().map(|n| n.value.clone()),
                                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                                };
                                for col_name in &pk.columns {
                                    if let Some(col) = table.columns.get_mut(col_name) {
                                        col.is_primary_key = true;
                                        col.nullable = false;
                                    }
                                }
                                table.primary_key = Some(pk);
                            }
                            TableConstraint::ForeignKey {
                                columns,
                                foreign_table,
                                referred_columns,
                                name,
                                ..
                            } => {
                                let fk = crate::schema::ForeignKeyDef {
                                    name: name.as_ref().map(|n| n.value.clone()),
                                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                                    references_table: object_name_to_qualified(foreign_table),
                                    references_columns: referred_columns
                                        .iter()
                                        .map(|c| c.value.clone())
                                        .collect(),
                                };
                                table.foreign_keys.push(fk);
                            }
                            TableConstraint::Unique { columns, name, .. } => {
                                let unique = UniqueConstraintDef {
                                    name: name.as_ref().map(|n| n.value.clone()),
                                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                                };
                                table.unique_constraints.push(unique);
                            }
                            TableConstraint::Check { name, expr, .. } => {
                                let check = CheckConstraintDef {
                                    name: name.as_ref().map(|n| n.value.clone()),
                                    expression: expr.to_string(),
                                };
                                table.check_constraints.push(check);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {
                    // Other ALTER TABLE operations - not yet supported
                }
            }
        }
    }

    /// Process CREATE TYPE statement
    fn process_create_type(
        &mut self,
        name: &ObjectName,
        representation: &UserDefinedTypeRepresentation,
    ) {
        let qualified = object_name_to_qualified(name);
        match representation {
            UserDefinedTypeRepresentation::Enum { labels } => {
                let enum_def = EnumTypeDef {
                    name: qualified.name,
                    values: labels.iter().map(|l| l.value.clone()).collect(),
                };
                self.catalog.add_enum(enum_def);
            }
            _ => {
                // Composite types and others - not yet supported
            }
        }
    }

    /// Process a column option (NOT NULL, DEFAULT, PRIMARY KEY, etc.)
    fn process_column_option(
        &mut self,
        col: &mut ColumnDef,
        table: &mut TableDef,
        option: &ColumnOptionDef,
    ) {
        match &option.option {
            ColumnOption::Null => {
                col.nullable = true;
            }
            ColumnOption::NotNull => {
                col.nullable = false;
            }
            ColumnOption::Default(expr) => {
                col.default = Some(expr_to_default(expr));
            }
            ColumnOption::Unique { is_primary, .. } => {
                if *is_primary {
                    col.is_primary_key = true;
                    col.nullable = false;
                }
            }
            ColumnOption::Check(expr) => {
                let check = CheckConstraintDef {
                    name: option.name.as_ref().map(|n| n.value.clone()),
                    expression: expr.to_string(),
                };
                table.check_constraints.push(check);
            }
            ColumnOption::Generated {
                generated_as,
                generation_expr,
                ..
            } => {
                // IDENTITY columns (no generation expression = IDENTITY, not computed)
                if generation_expr.is_none() {
                    use sqlparser::ast::GeneratedAs;
                    let kind = match generated_as {
                        GeneratedAs::Always => IdentityKind::Always,
                        GeneratedAs::ByDefault => IdentityKind::ByDefault,
                        _ => return,
                    };
                    col.identity = Some(kind);
                    col.nullable = false; // IDENTITY columns are implicitly NOT NULL
                }
            }
            ColumnOption::DialectSpecific(tokens) => {
                // MySQL AUTO_INCREMENT
                if tokens
                    .iter()
                    .any(|t| matches!(t, Token::Word(w) if w.value == "AUTO_INCREMENT"))
                {
                    col.nullable = false; // AUTO_INCREMENT implies NOT NULL
                }
            }
            _ => {}
        }
    }

    /// Process a table constraint (PRIMARY KEY, FOREIGN KEY, UNIQUE)
    fn process_table_constraint(&mut self, table: &mut TableDef, constraint: &TableConstraint) {
        match constraint {
            TableConstraint::PrimaryKey { columns, name, .. } => {
                let pk = PrimaryKeyDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                };
                // Mark columns as primary key
                for col_name in &pk.columns {
                    if let Some(col) = table.columns.get_mut(col_name) {
                        col.is_primary_key = true;
                        col.nullable = false;
                    }
                }
                table.primary_key = Some(pk);
            }
            TableConstraint::ForeignKey {
                columns,
                foreign_table,
                referred_columns,
                name,
                ..
            } => {
                let fk = ForeignKeyDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                    references_table: object_name_to_qualified(foreign_table),
                    references_columns: referred_columns.iter().map(|c| c.value.clone()).collect(),
                };
                table.foreign_keys.push(fk);
            }
            TableConstraint::Unique { columns, name, .. } => {
                let unique = UniqueConstraintDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                };
                table.unique_constraints.push(unique);
            }
            TableConstraint::Check { name, expr, .. } => {
                let check = CheckConstraintDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    expression: expr.to_string(),
                };
                table.check_constraints.push(check);
            }
            _ => {}
        }
    }

    /// Consume the builder and return the catalog
    pub fn build(self) -> (Catalog, Vec<Diagnostic>) {
        (self.catalog, self.diagnostics)
    }

    /// Get a reference to the current catalog
    #[allow(dead_code)]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

impl Default for SchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert sqlparser ObjectName to our QualifiedName
fn object_name_to_qualified(name: &ObjectName) -> QualifiedName {
    match name.0.as_slice() {
        [table] => QualifiedName::new(&table.value),
        [schema, table] => QualifiedName::with_schema(&schema.value, &table.value),
        [_catalog, schema, table] => QualifiedName::with_schema(&schema.value, &table.value),
        _ => QualifiedName::new(name.to_string()),
    }
}

/// Convert expression to DefaultValue
fn expr_to_default(expr: &sqlparser::ast::Expr) -> DefaultValue {
    match expr {
        sqlparser::ast::Expr::Value(v) => match v {
            sqlparser::ast::Value::Null => DefaultValue::Null,
            _ => DefaultValue::Literal(v.to_string()),
        },
        sqlparser::ast::Expr::Function(f) => {
            let func_name = f.name.to_string().to_lowercase();
            if func_name.contains("now") || func_name.contains("current_timestamp") {
                DefaultValue::CurrentTimestamp
            } else if func_name.contains("nextval") {
                DefaultValue::NextVal(f.to_string())
            } else {
                DefaultValue::Expression(f.to_string())
            }
        }
        _ => DefaultValue::Expression(expr.to_string()),
    }
}

/// Split SQL text into individual statements by semicolons,
/// respecting string literals and dollar-quoted strings.
fn split_sql_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'\'' => {
                // Skip single-quoted string
                i += 1;
                while i < len {
                    if bytes[i] == b'\'' {
                        i += 1;
                        if i < len && bytes[i] == b'\'' {
                            i += 1; // escaped quote ''
                        } else {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'$' => {
                // Check for dollar-quoted string ($$...$$ or $tag$...$tag$)
                if let Some(tag_end) = find_dollar_tag_end(sql, i) {
                    let tag = &sql[i..=tag_end];
                    i = tag_end + 1;
                    // Find the closing tag
                    if let Some(close_pos) = sql[i..].find(tag) {
                        i += close_pos + tag.len();
                    } else {
                        i = len; // unterminated, consume rest
                    }
                } else {
                    i += 1;
                }
            }
            b'-' if i + 1 < len && bytes[i + 1] == b'-' => {
                // Skip line comment
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                // Skip block comment
                i += 2;
                while i + 1 < len {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => {
                let stmt = &sql[start..i];
                if !stmt.trim().is_empty() {
                    statements.push(stmt);
                }
                start = i + 1;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Handle last statement (without trailing semicolon)
    let last = &sql[start..];
    if !last.trim().is_empty() {
        statements.push(last);
    }

    statements
}

/// Find the end of a dollar-quote tag starting at position `start`.
/// Returns the index of the closing `$` if a valid tag is found.
fn find_dollar_tag_end(sql: &str, start: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    // Tag is $<identifier>$ or just $$
    let mut i = start + 1;
    if i < len && bytes[i] == b'$' {
        return Some(i); // $$ tag
    }
    // Look for $identifier$
    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i < len && bytes[i] == b'$' {
        Some(i)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_table() {
        let sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT UNIQUE,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        let table = catalog.get_table(&QualifiedName::new("users")).unwrap();
        assert_eq!(table.columns.len(), 4);

        let id_col = table.get_column("id").unwrap();
        assert!(!id_col.nullable);
        assert!(id_col.is_primary_key);

        let name_col = table.get_column("name").unwrap();
        assert!(!name_col.nullable);
        assert!(matches!(name_col.data_type, SqlType::Varchar { .. }));

        let email_col = table.get_column("email").unwrap();
        assert!(email_col.nullable);
    }

    #[test]
    fn test_parse_table_with_foreign_key() {
        let sql = r#"
            CREATE TABLE orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id),
                total DECIMAL(10, 2)
            );
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        let table = catalog.get_table(&QualifiedName::new("orders")).unwrap();
        assert_eq!(table.columns.len(), 3);
    }

    #[test]
    fn test_split_sql_statements() {
        let sql = "CREATE TABLE a (id INT); CREATE TABLE b (id INT);";
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_split_preserves_string_literals() {
        let sql = "SELECT 'hello; world'; CREATE TABLE t (id INT);";
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("hello; world"));
    }

    #[test]
    fn test_parse_with_unsupported_statements() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURAL LANGUAGE plpgsql;

            CREATE TABLE actor (
                actor_id integer NOT NULL,
                first_name character varying(45) NOT NULL
            );

            ALTER TABLE public.actor OWNER TO postgres;

            CREATE TABLE category (
                category_id integer NOT NULL,
                name character varying(25) NOT NULL
            );
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        // Both tables should be found despite the unsupported PROCEDURAL LANGUAGE
        assert!(catalog.table_exists(&QualifiedName::new("actor")));
        assert!(catalog.table_exists(&QualifiedName::new("category")));
    }

    #[test]
    fn test_parse_sakila_like_schema() {
        // Simulates Sakila-style schema with mixed supported/unsupported statements
        let sql = r#"
            SET client_encoding = 'UTF8';
            SET standard_conforming_strings = off;

            COMMENT ON SCHEMA public IS 'Standard public schema';

            CREATE SEQUENCE actor_actor_id_seq
                INCREMENT BY 1
                NO MAXVALUE
                NO MINVALUE
                CACHE 1;

            CREATE TABLE actor (
                actor_id integer DEFAULT nextval('actor_actor_id_seq'::regclass) NOT NULL,
                first_name character varying(45) NOT NULL,
                last_name character varying(45) NOT NULL,
                last_update timestamp without time zone DEFAULT now() NOT NULL
            );

            ALTER TABLE public.actor OWNER TO postgres;

            CREATE TYPE mpaa_rating AS ENUM (
                'G', 'PG', 'PG-13', 'R', 'NC-17'
            );

            CREATE TABLE film (
                film_id integer NOT NULL,
                title character varying(255) NOT NULL,
                description text,
                release_year integer,
                rental_rate numeric(4,2) DEFAULT 4.99 NOT NULL,
                rating mpaa_rating DEFAULT 'G'
            );

            CREATE TABLE category (
                category_id integer NOT NULL,
                name character varying(25) NOT NULL,
                last_update timestamp without time zone DEFAULT now() NOT NULL
            );
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        assert!(
            catalog.table_exists(&QualifiedName::new("actor")),
            "actor table should exist"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("film")),
            "film table should exist"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("category")),
            "category table should exist"
        );
        assert!(
            catalog.enum_exists("mpaa_rating"),
            "mpaa_rating enum should exist"
        );
    }

    #[test]
    fn test_parse_with_functions_and_triggers() {
        let sql = r#"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE FUNCTION update_timestamp() RETURNS TRIGGER AS $$
            BEGIN
                NEW.updated_at = NOW();
                RETURN NEW;
            END;
            $$ LANGUAGE plpgsql;

            CREATE TABLE posts (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL,
                user_id INTEGER NOT NULL
            );
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        assert!(catalog.table_exists(&QualifiedName::new("users")));
        assert!(catalog.table_exists(&QualifiedName::new("posts")));
    }
}
