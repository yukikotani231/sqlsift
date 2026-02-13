//! SQL dialect support

use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use std::str::FromStr;

/// Supported SQL dialects
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlDialect {
    #[default]
    PostgreSQL,
    MySQL,
}

impl SqlDialect {
    /// Get the sqlparser dialect for parsing
    pub fn parser_dialect(&self) -> Box<dyn Dialect> {
        match self {
            SqlDialect::PostgreSQL => Box::new(PostgreSqlDialect {}),
            SqlDialect::MySQL => Box::new(MySqlDialect {}),
        }
    }

    /// Get default schema name for this dialect
    pub fn default_schema(&self) -> &'static str {
        match self {
            SqlDialect::PostgreSQL => "public",
            SqlDialect::MySQL => "",
        }
    }
}

impl FromStr for SqlDialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "postgresql" | "postgres" | "pg" => Ok(SqlDialect::PostgreSQL),
            "mysql" | "mysql8" => Ok(SqlDialect::MySQL),
            "sqlite" => Err(
                "SQLite dialect is not yet supported. Supported dialects: postgresql, mysql."
                    .to_string(),
            ),
            _ => Err(format!(
                "Unknown dialect: '{}'. Supported dialects: postgresql, mysql.",
                s
            )),
        }
    }
}

impl std::fmt::Display for SqlDialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqlDialect::PostgreSQL => write!(f, "postgresql"),
            SqlDialect::MySQL => write!(f, "mysql"),
        }
    }
}
