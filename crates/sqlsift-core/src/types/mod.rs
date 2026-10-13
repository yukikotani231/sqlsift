//! SQL type system

use serde::{Deserialize, Serialize};
use sqlsift_ast::{DataType, ExactNumberInfo};

use crate::ast_ext::ObjectNamePartExt;
use crate::dialect::SqlDialect;

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

    /// Semi-structured or spatial type whose values sqlsift doesn't model
    /// (Snowflake `VARIANT`, `OBJECT`, untyped `ARRAY`, `GEOGRAPHY`, `GEOMETRY`).
    /// Expressions of this type are never type checked. Holds the lowercase type name.
    SemiStructured(String),

    // Inline ENUM type with its allowed values (MySQL `ENUM('a', 'b')`)
    Enum(Vec<String>),

    // Custom/User-defined type
    Custom(String),

    // Unknown (when parsing fails)
    Unknown,
}

impl SqlType {
    /// Convert from sqlparser's DataType to our internal SqlType, with the type
    /// names only `dialect` has (Databricks `LONG`, `SHORT`, `BYTE`)
    pub fn from_ast_in(data_type: &DataType, dialect: SqlDialect) -> Self {
        match (SqlType::from_ast(data_type), dialect) {
            (SqlType::Custom(name), SqlDialect::Databricks) => {
                match name.to_ascii_lowercase().as_str() {
                    "long" => SqlType::BigInt,
                    "short" => SqlType::SmallInt,
                    "byte" => SqlType::TinyInt,
                    _ => SqlType::Custom(name),
                }
            }
            (ty, _) => ty,
        }
    }

    /// Convert from sqlparser's DataType to our internal SqlType
    pub fn from_ast(data_type: &DataType) -> Self {
        match data_type {
            DataType::TinyInt(_) | DataType::TinyIntUnsigned(_) => SqlType::TinyInt,
            DataType::SmallInt(_) | DataType::SmallIntUnsigned(_) => SqlType::SmallInt,
            DataType::Int2(_) | DataType::Int2Unsigned(_) => SqlType::SmallInt,
            DataType::MediumInt(_) | DataType::MediumIntUnsigned(_) => SqlType::MediumInt,
            DataType::Integer(_) | DataType::IntegerUnsigned(_) => SqlType::Integer,
            DataType::Int(_) | DataType::IntUnsigned(_) => SqlType::Integer,
            DataType::Int4(_) | DataType::Int4Unsigned(_) => SqlType::Integer,
            DataType::BigInt(_) | DataType::BigIntUnsigned(_) => SqlType::BigInt,
            DataType::Int8(_) | DataType::Int8Unsigned(_) => SqlType::BigInt,
            // BigQuery INT64 (and its aliases INT, INTEGER, BIGINT, ...)
            DataType::Int64 => SqlType::BigInt,

            DataType::Real | DataType::RealUnsigned => SqlType::Real,
            DataType::Float4 => SqlType::Real,
            DataType::Double(_) | DataType::DoubleUnsigned(_) => SqlType::DoublePrecision,
            DataType::DoublePrecision | DataType::DoublePrecisionUnsigned => {
                SqlType::DoublePrecision
            }
            DataType::Float8 => SqlType::DoublePrecision,
            DataType::Float32 => SqlType::Real,
            DataType::Float64 => SqlType::DoublePrecision,
            // FLOAT(p): 1..=24 is REAL, otherwise DOUBLE PRECISION (plain FLOAT is double)
            DataType::Float(ExactNumberInfo::Precision(p))
            | DataType::FloatUnsigned(ExactNumberInfo::Precision(p))
                if *p <= 24 =>
            {
                SqlType::Real
            }
            DataType::Float(_) | DataType::FloatUnsigned(_) => SqlType::DoublePrecision,

            // BigQuery BIGNUMERIC / BIGDECIMAL are wider NUMERICs
            DataType::Decimal(info)
            | DataType::DecimalUnsigned(info)
            | DataType::Numeric(info)
            | DataType::Dec(info)
            | DataType::DecUnsigned(info)
            | DataType::BigNumeric(info)
            | DataType::BigDecimal(info) => {
                let (precision, scale) = match info {
                    ExactNumberInfo::None => (None, None),
                    ExactNumberInfo::Precision(p) => (Some(*p), None),
                    // A negative scale (PostgreSQL 15+) is not modelled
                    ExactNumberInfo::PrecisionAndScale(p, s) => (Some(*p), u64::try_from(*s).ok()),
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

            DataType::Nvarchar(info) => SqlType::Varchar {
                length: extract_char_length(info.as_ref()),
            },

            DataType::Text => SqlType::Text,
            DataType::String(_) => SqlType::Text,
            DataType::TinyText | DataType::MediumText | DataType::LongText => SqlType::Text,
            DataType::Clob(_) => SqlType::Text,

            DataType::Bytea => SqlType::Bytea,
            DataType::Binary(_) | DataType::Varbinary(_) | DataType::Blob(_) => SqlType::Bytea,
            DataType::TinyBlob | DataType::MediumBlob | DataType::LongBlob => SqlType::Bytea,
            // BigQuery BYTES
            DataType::Bytes(_) => SqlType::Bytea,

            DataType::Date => SqlType::Date,

            DataType::Time(precision, tz) => SqlType::Time {
                precision: *precision,
                with_timezone: has_time_zone(*tz),
            },

            DataType::Timestamp(precision, tz) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: has_time_zone(*tz),
            },

            DataType::Datetime(precision) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: false,
            },

            // Snowflake TIMESTAMP_NTZ (no time zone)
            DataType::TimestampNtz(precision) => SqlType::Timestamp {
                precision: *precision,
                with_timezone: false,
            },

            DataType::Interval { .. } => SqlType::Interval,

            DataType::Boolean | DataType::Bool => SqlType::Boolean,

            DataType::Uuid => SqlType::Uuid,

            DataType::JSON => SqlType::Json,
            DataType::JSONB => SqlType::Jsonb,

            DataType::Enum(members, _) => SqlType::Enum(
                members
                    .iter()
                    .map(|m| match m {
                        sqlsift_ast::EnumMember::Name(name)
                        | sqlsift_ast::EnumMember::NamedValue(name, _) => name.clone(),
                    })
                    .collect(),
            ),

            DataType::Array(inner) => match inner {
                sqlsift_ast::ArrayElemTypeDef::AngleBracket(dt) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                sqlsift_ast::ArrayElemTypeDef::SquareBracket(dt, _) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                sqlsift_ast::ArrayElemTypeDef::Parenthesis(dt)
                | sqlsift_ast::ArrayElemTypeDef::Qualified(dt, _) => {
                    SqlType::Array(Box::new(SqlType::from_ast(dt)))
                }
                // A bare `ARRAY` is Snowflake's semi-structured array
                sqlsift_ast::ArrayElemTypeDef::None => SqlType::SemiStructured("array".to_string()),
            },

            // BigQuery STRUCT<...>: fields aren't modelled, so a struct is a type
            // sqlsift never reports (see `Custom`)
            DataType::Struct(..) => SqlType::Custom(data_type.to_string()),
            // Databricks MAP<K, V>, likewise
            DataType::Map(..) => SqlType::Custom("MAP".to_string()),

            // Types sqlparser reads into their own variants but that are user-defined
            // type names for sqlsift, as other extension types are (see `Custom`)
            DataType::TsVector
            | DataType::TsQuery
            | DataType::GeometricType(_)
            | DataType::VarBit(_)
            | DataType::HugeInt
            | DataType::UHugeInt
            | DataType::UTinyInt
            | DataType::USmallInt
            | DataType::UBigInt
            | DataType::Signed
            | DataType::SignedInteger
            | DataType::Unsigned
            | DataType::UnsignedInteger
            | DataType::AnyType => SqlType::Custom(data_type.to_string().to_lowercase()),

            DataType::Custom(name, modifiers) => {
                let type_name = name
                    .0
                    .iter()
                    .map(|i| i.ident().value.clone())
                    .collect::<Vec<_>>()
                    .join(".");
                let modifier = |i: usize| -> Option<u64> {
                    modifiers.get(i).and_then(|m| m.trim().parse().ok())
                };
                let lower = type_name.to_lowercase();
                match lower.as_str() {
                    // PostgreSQL serial types
                    "serial" | "serial4" => SqlType::Integer,
                    "bigserial" | "serial8" => SqlType::BigInt,
                    "smallserial" | "serial2" => SqlType::SmallInt,
                    // Snowflake (and Oracle) NUMBER(p, s)
                    "number" => SqlType::Decimal {
                        precision: modifier(0),
                        scale: modifier(1),
                    },
                    "byteint" => SqlType::TinyInt,
                    // Snowflake timestamps: no time zone, local time zone, time zone
                    "timestamp_ntz" => SqlType::Timestamp {
                        precision: modifier(0),
                        with_timezone: false,
                    },
                    "timestamp_ltz" | "timestamp_tz" => SqlType::Timestamp {
                        precision: modifier(0),
                        with_timezone: true,
                    },
                    // Snowflake semi-structured and spatial types
                    "variant" | "object" | "geography" | "geometry" => {
                        SqlType::SemiStructured(lower)
                    }
                    _ => SqlType::Custom(type_name),
                }
            }

            _ => SqlType::Unknown,
        }
    }

    /// Check if this type is compatible with another type
    pub fn is_compatible_with(&self, other: &SqlType) -> TypeCompatibility {
        use SqlType::*;

        if self == other {
            return TypeCompatibility::Exact;
        }

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

            // Semi-structured values (VARIANT, ...) hold values of any type
            (SemiStructured(_), _) | (_, SemiStructured(_)) => TypeCompatibility::ImplicitCast,

            // Inline ENUMs compare with strings (their values) and, in MySQL, with
            // numbers (their index); literal values are checked separately
            (Enum(_), _) | (_, Enum(_)) => TypeCompatibility::ImplicitCast,

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

    /// Whether sqlsift knows which arithmetic operators this type supports, so that
    /// using it with `+ - * / %` can be reported. User-defined and extension types
    /// (MONEY, domains, citext, dbt type placeholders, unknown warehouse types),
    /// semi-structured values and enums don't qualify, and neither do arrays and
    /// JSONB, which have arithmetic-looking operators in PostgreSQL (`jsonb - text`,
    /// intarray's `int[] - int`)
    pub fn has_known_arithmetic(&self) -> bool {
        !matches!(
            self,
            SqlType::Custom(_)
                | SqlType::SemiStructured(_)
                | SqlType::Enum(_)
                | SqlType::Array(_)
                | SqlType::Jsonb
                | SqlType::Unknown
        )
    }

    /// Check whether this is a character string type
    pub fn is_string(&self) -> bool {
        matches!(
            self,
            SqlType::Char { .. } | SqlType::Varchar { .. } | SqlType::Text
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

    /// Name of this type in the given dialect's terms, for diagnostics (BigQuery
    /// calls BIGINT `INT64`, TEXT `STRING`, ...); other dialects use
    /// [`Self::display_name`]
    pub fn display_name_in(&self, dialect: SqlDialect) -> String {
        if dialect != SqlDialect::BigQuery {
            return self.display_name();
        }
        match self {
            t if t.is_integer() => "INT64".to_string(),
            SqlType::Real | SqlType::DoublePrecision => "FLOAT64".to_string(),
            SqlType::Decimal { .. } => "NUMERIC".to_string(),
            SqlType::Char { .. } | SqlType::Varchar { .. } | SqlType::Text => "STRING".to_string(),
            SqlType::Bytea => "BYTES".to_string(),
            SqlType::Boolean => "BOOL".to_string(),
            SqlType::Time { .. } => "TIME".to_string(),
            SqlType::Timestamp { .. } => "TIMESTAMP".to_string(),
            SqlType::Json | SqlType::Jsonb => "JSON".to_string(),
            SqlType::Array(inner) => format!("ARRAY<{}>", inner.display_name_in(dialect)),
            SqlType::Custom(name) => name.clone(),
            other => other.display_name().to_uppercase(),
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
            SqlType::Enum(values) => format!(
                "enum({})",
                values
                    .iter()
                    .map(|v| format!("'{v}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            SqlType::SemiStructured(name) | SqlType::Custom(name) => name.clone(),
            SqlType::Unknown => "unknown".to_string(),
        }
    }
}

/// Whether a TIME/TIMESTAMP type carries a time zone (`WITH TIME ZONE`, or the `TIMESTAMPTZ` / `TIMETZ` shorthand)
fn has_time_zone(tz: sqlsift_ast::TimezoneInfo) -> bool {
    matches!(
        tz,
        sqlsift_ast::TimezoneInfo::WithTimeZone | sqlsift_ast::TimezoneInfo::Tz
    )
}

/// Extract character length from CharacterLength if present
fn extract_char_length(info: Option<&sqlsift_ast::CharacterLength>) -> Option<u64> {
    info.map(|i| match i {
        sqlsift_ast::CharacterLength::IntegerLength { length, .. } => *length,
        sqlsift_ast::CharacterLength::Max => u64::MAX,
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
