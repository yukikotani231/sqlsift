//! sqlsift-core: SQL static analysis library
//!
//! This library provides the core functionality for analyzing SQL queries
//! against schema definitions without requiring a database connection.

// Library code reports problems as diagnostics instead of panicking
#![warn(clippy::unwrap_used, clippy::expect_used)]

pub mod analyzer;
pub mod dialect;
pub mod error;
mod psql;
pub mod rules;
pub mod schema;
mod suggest;
pub mod types;

pub use analyzer::Analyzer;
pub use dialect::SqlDialect;
pub use error::{Diagnostic, DiagnosticKind, Severity, Span};
pub use rules::{RuleCategory, RuleConfig, RuleLevel};
pub use schema::{Catalog, ColumnDef, QualifiedName, Schema, TableDef};
pub use types::SqlType;
