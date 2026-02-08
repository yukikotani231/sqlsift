//! SQL dialect support

use sqlparser::dialect::{Dialect, PostgreSqlDialect};
use std::str::FromStr;

/// Supported SQL dialects
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlDialect {
    #[default]
    PostgreSQL,
}

impl SqlDialect {
    /// Get the sqlparser dialect for parsing
    pub fn parser_dialect(&self) -> Box<dyn Dialect> {
        match self {
            SqlDialect::PostgreSQL => Box::new(PostgreSqlDialect {}),
        }
    }

    /// Get default schema name for this dialect
    pub fn default_schema(&self) -> &'static str {
        match self {
            SqlDialect::PostgreSQL => "public",
        }
    }
}

impl FromStr for SqlDialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "postgresql" | "postgres" | "pg" => Ok(SqlDialect::PostgreSQL),
            "mysql" => Err(
                "MySQL dialect is not yet supported. Currently only PostgreSQL is supported."
                    .to_string(),
            ),
            "sqlite" => Err(
                "SQLite dialect is not yet supported. Currently only PostgreSQL is supported."
                    .to_string(),
            ),
            _ => Err(format!(
                "Unknown dialect: '{}'. Currently only PostgreSQL is supported.",
                s
            )),
        }
    }
}

impl std::fmt::Display for SqlDialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqlDialect::PostgreSQL => write!(f, "postgresql"),
        }
    }
}
