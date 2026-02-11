//! SQL type system

use serde::{Deserialize, Serialize};
use sqlparser::ast::DataType;

/// Internal representation of SQL types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SqlType {
    // Numeric types
    TinyInt,
    SmallInt,
    MediumInt,
    Integer,
    BigInt,
    Decimal {
        precision: Option<u64>,
        scale: Option<u64>,
    },
    Real,
    DoublePrecision,

    // Character types
    Char {
        length: Option<u64>,
    },
    Varchar {
        length: Option<u64>,
    },
    Text,

    // Binary types
    Bytea,

    // Date/Time types
    Date,
    Time {
        precision: Option<u64>,
        with_timezone: bool,
    },
    Timestamp {
        precision: Option<u64>,
        with_timezone: bool,
    },
    Interval,

    // Boolean
    Boolean,

    // UUID
    Uuid,

    // JSON
    Json,
    Jsonb,

    // Array
    Array(Box<SqlType>),

    // Custom/User-defined type
    Custom(String),

    // Unknown (when parsing fails)
    Unknown,
}

impl SqlType {
    /// Convert from sqlparser's DataType to our internal SqlType
    pub fn from_ast(data_type: &DataType) -> Self {
        match data_type {
            DataType::TinyInt(_) | DataType::UnsignedTinyInt(_) => SqlType::TinyInt,
            DataType::SmallInt(_) | DataType::UnsignedSmallInt(_) => SqlType::SmallInt,
            DataType::Int2(_) => SqlType::SmallInt,
            DataType::MediumInt(_) | DataType::UnsignedMediumInt(_) => SqlType::MediumInt,
            DataType::Integer(_) | DataType::UnsignedInteger(_) => SqlType::Integer,
            DataType::Int(_) | DataType::UnsignedInt(_) => SqlType::Integer,
            DataType::Int4(_) => SqlType::Integer,
            DataType::BigInt(_) | DataType::UnsignedBigInt(_) => SqlType::BigInt,
            DataType::Int8(_) => SqlType::BigInt,

            DataType::Real => SqlType::Real,
            DataType::Float4 => SqlType::Real,
            DataType::Double => SqlType::DoublePrecision,
            DataType::DoublePrecision => SqlType::DoublePrecision,
            DataType::Float8 => SqlType::DoublePrecision,

            DataType::Decimal(info) | DataType::Numeric(info) => {
                let (precision, scale) = match info {
                    sqlparser::ast::ExactNumberInfo::None => (None, None),
                    sqlparser::ast::ExactNumberInfo::Precision(p) => (Some(*p), None),
                    sqlparser::ast::ExactNumberInfo::PrecisionAndScale(p, s) => {
                        (Some(*p), Some(*s))
                    }
                };
                SqlType::Decimal { precision, scale }
            }

            DataType::Char(info) | DataType::Character(info) => {
                let length = extract_char_length(info.as_ref());
                SqlType::Char { length }
            }

            DataType::Varchar(info) | DataType::CharacterVarying(info) => {
                let length = extract_char_length(info.as_ref());
                SqlType::Varchar { length }
            }

            DataType::Text => SqlType::Text,
            DataType::String(_) => SqlType::Text,

            DataType::Bytea => SqlType::Bytea,
            DataType::Binary(_) | DataType::Varbinary(_) | DataType::Blob(_) => SqlType::Bytea,

            DataType::Date => SqlType::Date,

            DataType::Time(precision, tz) => SqlType::Time {
                precision: *precision,
                with_timezone: matches!(tz, sqlparser::ast::TimezoneInfo::WithTimeZone),
            },

            DataType::Timestamp(precision, tz) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: matches!(tz, sqlparser::ast::TimezoneInfo::WithTimeZone),
            },

            DataType::Datetime(precision) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: false,
            },

            DataType::Interval => SqlType::Interval,

            DataType::Boolean | DataType::Bool => SqlType::Boolean,

            DataType::Uuid => SqlType::Uuid,

            DataType::JSON => SqlType::Json,
            DataType::JSONB => SqlType::Jsonb,

            DataType::Enum(..) => SqlType::Custom("ENUM".to_string()),

            DataType::Array(inner) => match inner {
                sqlparser::ast::ArrayElemTypeDef::AngleBracket(dt) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                sqlparser::ast::ArrayElemTypeDef::SquareBracket(dt, _) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                sqlparser::ast::ArrayElemTypeDef::Parenthesis(dt) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                sqlparser::ast::ArrayElemTypeDef::None => {
                    SqlType::Array(Box::new(SqlType::Unknown))
                }
            },

            DataType::Custom(name, _) => {
                let type_name = name
                    .0
                    .iter()
                    .map(|i| i.value.clone())
                    .collect::<Vec<_>>()
                    .join(".");
                // Handle common PostgreSQL type aliases
                match type_name.to_lowercase().as_str() {
                    "serial" | "serial4" => SqlType::Integer,
                    "bigserial" | "serial8" => SqlType::BigInt,
                    "smallserial" | "serial2" => SqlType::SmallInt,
                    _ => SqlType::Custom(type_name),
                }
            }

            _ => SqlType::Unknown,
        }
    }

    /// Check if this type is compatible with another type
    pub fn is_compatible_with(&self, other: &SqlType) -> TypeCompatibility {
        if self == other {
            return TypeCompatibility::Exact;
        }

        use SqlType::*;
        match (self, other) {
            // Numeric type coercion
            (TinyInt, SmallInt | MediumInt | Integer | BigInt) => TypeCompatibility::ImplicitCast,
            (SmallInt, MediumInt | Integer | BigInt) => TypeCompatibility::ImplicitCast,
            (MediumInt, Integer | BigInt) => TypeCompatibility::ImplicitCast,
            (Integer, BigInt) => TypeCompatibility::ImplicitCast,
            (TinyInt | SmallInt | MediumInt | Integer | BigInt, Real | DoublePrecision) => {
                TypeCompatibility::ImplicitCast
            }
            (Real, DoublePrecision) => TypeCompatibility::ImplicitCast,
            (TinyInt | SmallInt | MediumInt | Integer | BigInt, Decimal { .. }) => {
                TypeCompatibility::ImplicitCast
            }

            // String type coercion
            (Char { .. }, Varchar { .. } | Text) => TypeCompatibility::ImplicitCast,
            (Varchar { .. }, Text) => TypeCompatibility::ImplicitCast,

            // JSON coercion
            (Json, Jsonb) => TypeCompatibility::ImplicitCast,

            // String to UUID coercion (PostgreSQL implicit cast)
            (Char { .. } | Varchar { .. } | Text, Uuid) => TypeCompatibility::ImplicitCast,

            // Any type can be explicitly cast
            _ => TypeCompatibility::ExplicitCast,
        }
    }

    /// Get a human-readable name for this type
    pub fn display_name(&self) -> String {
        match self {
            SqlType::TinyInt => "tinyint".to_string(),
            SqlType::SmallInt => "smallint".to_string(),
            SqlType::MediumInt => "mediumint".to_string(),
            SqlType::Integer => "integer".to_string(),
            SqlType::BigInt => "bigint".to_string(),
            SqlType::Decimal { precision, scale } => match (precision, scale) {
                (Some(p), Some(s)) => format!("numeric({p},{s})"),
                (Some(p), None) => format!("numeric({p})"),
                _ => "numeric".to_string(),
            },
            SqlType::Real => "real".to_string(),
            SqlType::DoublePrecision => "double precision".to_string(),
            SqlType::Char { length } => match length {
                Some(l) => format!("char({l})"),
                None => "char".to_string(),
            },
            SqlType::Varchar { length } => match length {
                Some(l) => format!("varchar({l})"),
                None => "varchar".to_string(),
            },
            SqlType::Text => "text".to_string(),
            SqlType::Bytea => "bytea".to_string(),
            SqlType::Date => "date".to_string(),
            SqlType::Time {
                with_timezone: true,
                ..
            } => "time with time zone".to_string(),
            SqlType::Time { .. } => "time".to_string(),
            SqlType::Timestamp {
                with_timezone: true,
                ..
            } => "timestamp with time zone".to_string(),
            SqlType::Timestamp { .. } => "timestamp".to_string(),
            SqlType::Interval => "interval".to_string(),
            SqlType::Boolean => "boolean".to_string(),
            SqlType::Uuid => "uuid".to_string(),
            SqlType::Json => "json".to_string(),
            SqlType::Jsonb => "jsonb".to_string(),
            SqlType::Array(inner) => format!("{}[]", inner.display_name()),
            SqlType::Custom(name) => name.clone(),
            SqlType::Unknown => "unknown".to_string(),
        }
    }
}

/// Extract character length from CharacterLength if present
fn extract_char_length(info: Option<&sqlparser::ast::CharacterLength>) -> Option<u64> {
    info.map(|i| match i {
        sqlparser::ast::CharacterLength::IntegerLength { length, .. } => *length,
        sqlparser::ast::CharacterLength::Max => u64::MAX,
    })
}

/// Result of type compatibility check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCompatibility {
    /// Types are exactly the same
    Exact,
    /// Implicit cast is possible
    ImplicitCast,
    /// Explicit cast is required
    ExplicitCast,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_compatibility() {
        assert_eq!(
            SqlType::SmallInt.is_compatible_with(&SqlType::Integer),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Integer.is_compatible_with(&SqlType::Integer),
            TypeCompatibility::Exact
        );
    }

    #[test]
    fn test_string_to_uuid_implicit_cast() {
        // PostgreSQL allows implicit cast from string literals to UUID
        assert_eq!(
            SqlType::Text.is_compatible_with(&SqlType::Uuid),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Varchar { length: Some(36) }.is_compatible_with(&SqlType::Uuid),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Char { length: Some(36) }.is_compatible_with(&SqlType::Uuid),
            TypeCompatibility::ImplicitCast
        );
        // UUID to string requires explicit cast
        assert_eq!(
            SqlType::Uuid.is_compatible_with(&SqlType::Text),
            TypeCompatibility::ExplicitCast
        );
    }
}
