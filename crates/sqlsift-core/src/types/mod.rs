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
                with_timezone: has_time_zone(tz),
            },

            DataType::Timestamp(precision, tz) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: has_time_zone(tz),
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
            // NUMERIC with different precision/scale, and NUMERIC → floating point
            (Decimal { .. }, Decimal { .. } | Real | DoublePrecision) => {
                TypeCompatibility::ImplicitCast
            }

            // String type coercion (including different lengths)
            (Char { .. }, Char { .. } | Varchar { .. } | Text) => TypeCompatibility::ImplicitCast,
            (Varchar { .. }, Varchar { .. } | Text) => TypeCompatibility::ImplicitCast,

            // Date/time coercion: precision and time zone differences are implicit,
            // and DATE widens to TIMESTAMP
            (Timestamp { .. }, Timestamp { .. }) => TypeCompatibility::ImplicitCast,
            (Time { .. }, Time { .. }) => TypeCompatibility::ImplicitCast,
            (Date, Timestamp { .. }) => TypeCompatibility::ImplicitCast,

            // JSON coercion
            (Json, Jsonb) => TypeCompatibility::ImplicitCast,

            // String to UUID coercion (PostgreSQL implicit cast)
            (Char { .. } | Varchar { .. } | Text, Uuid) => TypeCompatibility::ImplicitCast,

            // String to ENUM coercion (ENUM values are string literals)
            (Char { .. } | Varchar { .. } | Text, Custom(name)) if name == "ENUM" => {
                TypeCompatibility::ImplicitCast
            }

            // Other user-defined types (domains, extension types such as citext, ...)
            // carry no information about their casts, so don't report them
            (Custom(_), _) | (_, Custom(_)) => TypeCompatibility::ImplicitCast,

            // Any type can be explicitly cast
            _ => TypeCompatibility::ExplicitCast,
        }
    }

    /// Check whether a quoted string literal can be implicitly converted to this type.
    ///
    /// String literals are untyped until they meet another operand, so `'2024-01-01'`
    /// is a valid DATE and `'42'` a valid INTEGER. Only numeric and boolean literals are
    /// validated; for other types (dates, JSON, arrays, enums, ...) the database's input
    /// format is too permissive to check reliably, so the literal is accepted.
    pub fn accepts_string_literal(&self, literal: &str) -> bool {
        let value = literal.trim();
        match self {
            SqlType::TinyInt
            | SqlType::SmallInt
            | SqlType::MediumInt
            | SqlType::Integer
            | SqlType::BigInt => value.parse::<i128>().is_ok(),
            SqlType::Decimal { .. } | SqlType::Real | SqlType::DoublePrecision => {
                value.parse::<f64>().is_ok()
            }
            SqlType::Boolean => matches!(
                value.to_ascii_lowercase().as_str(),
                "t" | "true" | "f" | "false" | "y" | "yes" | "n" | "no" | "on" | "off" | "1" | "0"
            ),
            _ => true,
        }
    }

    /// Check whether this is a numeric type
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            SqlType::TinyInt
                | SqlType::SmallInt
                | SqlType::MediumInt
                | SqlType::Integer
                | SqlType::BigInt
                | SqlType::Real
                | SqlType::DoublePrecision
                | SqlType::Decimal { .. }
        )
    }

    /// Check whether this is an integer type
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            SqlType::TinyInt
                | SqlType::SmallInt
                | SqlType::MediumInt
                | SqlType::Integer
                | SqlType::BigInt
        )
    }

    /// Result type of date/time arithmetic (`left op right`), following PostgreSQL rules.
    ///
    /// Returns `None` if the operands are not a valid date/time combination.
    pub fn temporal_arithmetic_result(
        left: &SqlType,
        op: ArithmeticOp,
        right: &SqlType,
    ) -> Option<SqlType> {
        use ArithmeticOp::*;
        use SqlType::*;
        let timestamp = Timestamp {
            precision: None,
            with_timezone: false,
        };
        match (left, op, right) {
            (Timestamp { .. }, Add | Subtract, Interval) | (Interval, Add, Timestamp { .. }) => {
                let ts = if matches!(left, Timestamp { .. }) {
                    left
                } else {
                    right
                };
                Some(ts.clone())
            }
            (Timestamp { .. }, Subtract, Timestamp { .. } | Date) => Some(Interval),
            (Date, Subtract, Timestamp { .. }) => Some(Interval),
            (Date, Add | Subtract, Interval) | (Interval, Add, Date) => Some(timestamp),
            (Date, Add, Time { .. }) | (Time { .. }, Add, Date) => Some(timestamp),
            (Date, Add | Subtract, r) if r.is_integer() => Some(Date),
            (l, Add, Date) if l.is_integer() => Some(Date),
            (Date, Subtract, Date) => Some(Integer),
            (Time { .. }, Add | Subtract, Interval) => Some(left.clone()),
            (Interval, Add, Time { .. }) => Some(right.clone()),
            (Time { .. }, Subtract, Time { .. }) => Some(Interval),
            (Interval, Add | Subtract, Interval) => Some(Interval),
            (Interval, Multiply | Divide, r) if r.is_numeric() => Some(Interval),
            (l, Multiply, Interval) if l.is_numeric() => Some(Interval),
            _ => None,
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

/// Whether a TIME/TIMESTAMP type carries a time zone (`WITH TIME ZONE`, or the `TIMESTAMPTZ` / `TIMETZ` shorthand)
fn has_time_zone(tz: &sqlparser::ast::TimezoneInfo) -> bool {
    matches!(
        tz,
        sqlparser::ast::TimezoneInfo::WithTimeZone | sqlparser::ast::TimezoneInfo::Tz
    )
}

/// Extract character length from CharacterLength if present
fn extract_char_length(info: Option<&sqlparser::ast::CharacterLength>) -> Option<u64> {
    info.map(|i| match i {
        sqlparser::ast::CharacterLength::IntegerLength { length, .. } => *length,
        sqlparser::ast::CharacterLength::Max => u64::MAX,
    })
}

/// Arithmetic operator, used for date/time arithmetic rules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
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
    fn test_parameterized_types_are_compatible() {
        let ts = |tz| SqlType::Timestamp {
            precision: None,
            with_timezone: tz,
        };
        assert_eq!(
            ts(false).is_compatible_with(&ts(true)),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Date.is_compatible_with(&ts(true)),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Varchar { length: Some(10) }
                .is_compatible_with(&SqlType::Varchar { length: Some(20) }),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Decimal {
                precision: Some(10),
                scale: Some(2)
            }
            .is_compatible_with(&SqlType::Decimal {
                precision: None,
                scale: None
            }),
            TypeCompatibility::ImplicitCast
        );
        assert_eq!(
            SqlType::Integer.is_compatible_with(&SqlType::Boolean),
            TypeCompatibility::ExplicitCast
        );
    }

    #[test]
    fn test_accepts_string_literal() {
        assert!(SqlType::Integer.accepts_string_literal("42"));
        assert!(SqlType::BigInt.accepts_string_literal(" -7 "));
        assert!(!SqlType::Integer.accepts_string_literal("abc"));
        assert!(!SqlType::Integer.accepts_string_literal("1.5"));
        assert!(SqlType::Decimal {
            precision: None,
            scale: None
        }
        .accepts_string_literal("1.5"));
        assert!(SqlType::Boolean.accepts_string_literal("TRUE"));
        assert!(!SqlType::Boolean.accepts_string_literal("maybe"));
        assert!(SqlType::Date.accepts_string_literal("2024-01-01"));
        assert!(SqlType::Jsonb.accepts_string_literal("{}"));
    }

    #[test]
    fn test_temporal_arithmetic() {
        let tstz = SqlType::Timestamp {
            precision: None,
            with_timezone: true,
        };
        assert_eq!(
            SqlType::temporal_arithmetic_result(&tstz, ArithmeticOp::Subtract, &SqlType::Interval),
            Some(tstz.clone())
        );
        assert_eq!(
            SqlType::temporal_arithmetic_result(&tstz, ArithmeticOp::Subtract, &tstz),
            Some(SqlType::Interval)
        );
        assert_eq!(
            SqlType::temporal_arithmetic_result(
                &SqlType::Date,
                ArithmeticOp::Add,
                &SqlType::Integer
            ),
            Some(SqlType::Date)
        );
        assert_eq!(
            SqlType::temporal_arithmetic_result(
                &SqlType::Date,
                ArithmeticOp::Subtract,
                &SqlType::Date
            ),
            Some(SqlType::Integer)
        );
        assert_eq!(
            SqlType::temporal_arithmetic_result(&tstz, ArithmeticOp::Add, &tstz),
            None
        );
        assert_eq!(
            SqlType::temporal_arithmetic_result(
                &SqlType::Text,
                ArithmeticOp::Add,
                &SqlType::Interval
            ),
            None
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
