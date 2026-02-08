//! sqlsurge-core: SQL static analysis library
//!
//! This library provides the core functionality for analyzing SQL queries
//! against schema definitions without requiring a database connection.

pub mod analyzer;
pub mod dialect;
pub mod error;
pub mod schema;
pub mod types;

pub use analyzer::Analyzer;
pub use dialect::SqlDialect;
pub use error::{Diagnostic, DiagnosticKind, Severity, Span};
pub use schema::{Catalog, ColumnDef, QualifiedName, Schema, TableDef};
pub use types::SqlType;
