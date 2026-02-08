//! SQL analyzer module

mod resolver;
mod type_resolver;

use sqlparser::parser::Parser;

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::Catalog;

pub use resolver::NameResolver;
use type_resolver::TypeResolver;

/// SQL Analyzer - validates SQL against a schema catalog
pub struct Analyzer<'a> {
    catalog: &'a Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
}

impl<'a> Analyzer<'a> {
    /// Create a new analyzer with default PostgreSQL dialect
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsurge_core::analyzer::Analyzer;
    /// use sqlsurge_core::schema::Catalog;
    ///
    /// let catalog = Catalog::default();
    /// let mut analyzer = Analyzer::new(&catalog);
    /// ```
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect: SqlDialect::default(),
        }
    }

    /// Create a new analyzer with specified SQL dialect
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsurge_core::analyzer::Analyzer;
    /// use sqlsurge_core::dialect::SqlDialect;
    /// use sqlsurge_core::schema::Catalog;
    ///
    /// let catalog = Catalog::default();
    /// let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);
    /// ```
    pub fn with_dialect(catalog: &'a Catalog, dialect: SqlDialect) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect,
        }
    }

    /// Analyze a SQL query and return diagnostics
    ///
    /// Validates SQL against the schema catalog and returns a list of diagnostics.
    /// Returns an empty vector if no issues are found.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsurge_core::analyzer::Analyzer;
    /// use sqlsurge_core::schema::{Catalog, SchemaBuilder};
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let mut analyzer = Analyzer::new(&catalog);
    /// let diagnostics = analyzer.analyze("SELECT id, name FROM users");
    /// assert!(diagnostics.is_empty());
    /// ```
    pub fn analyze(&mut self, sql: &str) -> Vec<Diagnostic> {
        self.diagnostics.clear();

        // Parse the SQL
        let dialect = self.dialect.parser_dialect();
        let statements = match Parser::parse_sql(dialect.as_ref(), sql) {
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
            // Phase 1: Name resolution
            let mut resolver = NameResolver::new(self.catalog);
            resolver.resolve_statement(stmt);

            // Phase 2: Type inference and checking
            let mut type_resolver = TypeResolver::new(self.catalog);
            type_resolver.inherit_scope(&resolver);
            type_resolver.check_statement(stmt);

            // Collect diagnostics from both phases
            self.diagnostics.extend(resolver.into_diagnostics());
            self.diagnostics.extend(type_resolver.into_diagnostics());
        }

        std::mem::take(&mut self.diagnostics)
    }
}
