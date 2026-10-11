//! SQL dialect support

use sqlparser::dialect::{
    BigQueryDialect, DatabricksDialect, Dialect, MySqlDialect, PostgreSqlDialect,
    RedshiftSqlDialect, SQLiteDialect, SnowflakeDialect,
};
use std::str::FromStr;

/// Supported SQL dialects
///
/// The data warehouse dialects (Snowflake, BigQuery, Redshift, Databricks) are
/// alpha: queries parse with the warehouse's syntax and names are resolved, but
/// many warehouse types and functions are still unknown to sqlsift (and so never
/// reported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlDialect {
    #[default]
    PostgreSQL,
    MySQL,
    SQLite,
    /// Snowflake (alpha)
    Snowflake,
    /// Google BigQuery (alpha)
    BigQuery,
    /// Amazon Redshift (alpha)
    Redshift,
    /// Databricks / Spark SQL (alpha)
    Databricks,
}

impl SqlDialect {
    /// Every dialect, in the order they are listed to users
    pub const ALL: [SqlDialect; 7] = [
        SqlDialect::PostgreSQL,
        SqlDialect::MySQL,
        SqlDialect::SQLite,
        SqlDialect::Snowflake,
        SqlDialect::BigQuery,
        SqlDialect::Redshift,
        SqlDialect::Databricks,
    ];

    /// Get the sqlparser dialect for parsing
    pub fn parser_dialect(&self) -> Box<dyn Dialect> {
        match self {
            SqlDialect::PostgreSQL => Box::new(PostgreSqlDialect {}),
            SqlDialect::MySQL => Box::new(MySqlDialect {}),
            SqlDialect::SQLite => Box::new(SQLiteDialect {}),
            SqlDialect::Snowflake => Box::new(SnowflakeDialect {}),
            SqlDialect::BigQuery => Box::new(BigQueryDialect {}),
            SqlDialect::Redshift => Box::new(RedshiftSqlDialect {}),
            SqlDialect::Databricks => Box::new(DatabricksDialect {}),
        }
    }

    /// Get default schema name for this dialect
    pub fn default_schema(&self) -> &'static str {
        match self {
            SqlDialect::PostgreSQL | SqlDialect::Redshift => "public",
            SqlDialect::Snowflake => "PUBLIC",
            SqlDialect::MySQL
            | SqlDialect::SQLite
            | SqlDialect::BigQuery
            | SqlDialect::Databricks => "",
        }
    }

    /// Whether support for this dialect is still alpha (a data warehouse dialect)
    pub fn is_alpha(self) -> bool {
        matches!(
            self,
            SqlDialect::Snowflake
                | SqlDialect::BigQuery
                | SqlDialect::Redshift
                | SqlDialect::Databricks
        )
    }

    /// Whether referencing a column name that a derived table or CTE outputs more
    /// than once (`SELECT *` over a join) is an error. SQLite resolves it to the
    /// first such column; MySQL already rejects the derived table itself. Left off
    /// for Snowflake and Databricks until their behaviour is verified
    pub fn rejects_duplicate_column_references(self) -> bool {
        !matches!(
            self,
            SqlDialect::SQLite | SqlDialect::Snowflake | SqlDialect::Databricks
        )
    }

    /// Whether booleans are integers (MySQL BOOLEAN is TINYINT(1), SQLite stores
    /// booleans as integers), so integers and booleans compare without a cast
    pub fn has_integer_booleans(self) -> bool {
        matches!(self, SqlDialect::MySQL | SqlDialect::SQLite)
    }

    /// Whether character strings are implicitly coerced to numbers, dates,
    /// timestamps and booleans (and numbers to booleans) when compared, assigned or
    /// used in arithmetic: Snowflake converts the value at run time, so only a
    /// string literal that can't be converted is a definite error
    pub fn coerces_strings(self) -> bool {
        matches!(self, SqlDialect::Snowflake)
    }

    /// Whether inserting NULL into a single-column integer primary key generates
    /// a key (MySQL AUTO_INCREMENT, SQLite rowid alias)
    pub fn null_generates_integer_key(self) -> bool {
        matches!(self, SqlDialect::MySQL | SqlDialect::SQLite)
    }

    /// Whether HAVING can reference output column aliases (PostgreSQL's can't)
    pub fn aliases_in_having(self) -> bool {
        !matches!(self, SqlDialect::PostgreSQL)
    }

    /// Whether output column aliases are visible in WHERE and in later items of
    /// the SELECT list ("lateral column aliases": `SELECT a + 1 AS b, b * 2 AS c
    /// FROM t WHERE b > 0`)
    pub fn lateral_column_aliases(self) -> bool {
        matches!(
            self,
            SqlDialect::Snowflake | SqlDialect::Redshift | SqlDialect::Databricks
        )
    }

    /// Whether backticks quote identifiers
    pub fn backtick_identifiers(self) -> bool {
        matches!(
            self,
            SqlDialect::MySQL | SqlDialect::SQLite | SqlDialect::BigQuery | SqlDialect::Databricks
        )
    }

    /// Whether `a.b.c` can be a path into a STRUCT column (BigQuery, Databricks):
    /// `a` may be a column rather than a table, and the parts after the column are
    /// field names
    pub fn struct_field_paths(self) -> bool {
        matches!(self, SqlDialect::BigQuery | SqlDialect::Databricks)
    }

    /// Whether query text may contain psql meta-commands and `:variables`
    /// (rewritten by [`crate::psql::preprocess`])
    pub(crate) fn is_psql(self) -> bool {
        matches!(self, SqlDialect::PostgreSQL | SqlDialect::Redshift)
    }

    /// A positional bind parameter placeholder
    pub(crate) fn bind_parameter(self) -> &'static str {
        match self {
            SqlDialect::PostgreSQL | SqlDialect::Redshift => "$1",
            SqlDialect::MySQL
            | SqlDialect::SQLite
            | SqlDialect::Snowflake
            | SqlDialect::BigQuery
            | SqlDialect::Databricks => "?",
        }
    }
}

impl FromStr for SqlDialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "postgresql" | "postgres" | "pg" => Ok(SqlDialect::PostgreSQL),
            "mysql" | "mysql8" => Ok(SqlDialect::MySQL),
            "sqlite" | "sqlite3" => Ok(SqlDialect::SQLite),
            "snowflake" => Ok(SqlDialect::Snowflake),
            "bigquery" | "bq" => Ok(SqlDialect::BigQuery),
            "redshift" => Ok(SqlDialect::Redshift),
            "databricks" | "spark" | "sparksql" => Ok(SqlDialect::Databricks),
            _ => Err(format!(
                "Unknown dialect: '{s}'. Supported dialects: postgresql, mysql, sqlite, \
                 snowflake, bigquery, redshift, databricks."
            )),
        }
    }
}

impl std::fmt::Display for SqlDialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqlDialect::PostgreSQL => write!(f, "postgresql"),
            SqlDialect::MySQL => write!(f, "mysql"),
            SqlDialect::SQLite => write!(f, "sqlite"),
            SqlDialect::Snowflake => write!(f, "snowflake"),
            SqlDialect::BigQuery => write!(f, "bigquery"),
            SqlDialect::Redshift => write!(f, "redshift"),
            SqlDialect::Databricks => write!(f, "databricks"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_round_trips_through_its_name() {
        for dialect in SqlDialect::ALL {
            assert_eq!(dialect.to_string().parse(), Ok(dialect));
        }
    }

    #[test]
    fn warehouse_aliases_parse() {
        assert_eq!("bq".parse(), Ok(SqlDialect::BigQuery));
        assert_eq!("Spark".parse(), Ok(SqlDialect::Databricks));
        assert!(SqlDialect::Snowflake.is_alpha());
        assert!(!SqlDialect::PostgreSQL.is_alpha());
    }
}
