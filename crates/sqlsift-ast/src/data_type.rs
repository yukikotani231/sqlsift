//! Data types, as written.

use crate::{Expr, ObjectName};

/// A data type as written. Types the analyzer maps to nothing in particular are
/// [`DataType::Other`], with their text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DataType {
    Character(Option<CharacterLength>),
    Char(Option<CharacterLength>),
    CharacterVarying(Option<CharacterLength>),
    Varchar(Option<CharacterLength>),
    Nvarchar(Option<CharacterLength>),
    Uuid,
    Clob(Option<u64>),
    Binary(Option<u64>),
    Varbinary(Option<u64>),
    Blob(Option<u64>),
    TinyBlob,
    MediumBlob,
    LongBlob,
    Bytes(Option<u64>),
    Numeric(ExactNumberInfo),
    Decimal(ExactNumberInfo),
    DecimalUnsigned(ExactNumberInfo),
    BigNumeric(ExactNumberInfo),
    BigDecimal(ExactNumberInfo),
    Dec(ExactNumberInfo),
    DecUnsigned(ExactNumberInfo),
    Float(ExactNumberInfo),
    FloatUnsigned(ExactNumberInfo),
    TinyInt(Option<u64>),
    TinyIntUnsigned(Option<u64>),
    Int2(Option<u64>),
    Int2Unsigned(Option<u64>),
    SmallInt(Option<u64>),
    SmallIntUnsigned(Option<u64>),
    MediumInt(Option<u64>),
    MediumIntUnsigned(Option<u64>),
    Int(Option<u64>),
    Int4(Option<u64>),
    Int8(Option<u64>),
    Int64,
    Integer(Option<u64>),
    IntUnsigned(Option<u64>),
    Int4Unsigned(Option<u64>),
    IntegerUnsigned(Option<u64>),
    BigInt(Option<u64>),
    BigIntUnsigned(Option<u64>),
    Int8Unsigned(Option<u64>),
    Float4,
    Float32,
    Float64,
    Real,
    RealUnsigned,
    Float8,
    Double(ExactNumberInfo),
    DoubleUnsigned(ExactNumberInfo),
    DoublePrecision,
    DoublePrecisionUnsigned,
    Bool,
    Boolean,
    Date,
    Time(Option<u64>, TimezoneInfo),
    Datetime(Option<u64>),
    Timestamp(Option<u64>, TimezoneInfo),
    TimestampNtz(Option<u64>),
    /// `INTERVAL [fields] [(precision)]`: its text after `INTERVAL`
    Interval(String),
    JSON,
    JSONB,
    Text,
    TinyText,
    MediumText,
    LongText,
    String(Option<u64>),
    Bytea,
    /// A type name the grammar doesn't know, with its modifiers (`NUMBER(10, 2)`)
    Custom(ObjectName, Vec<String>),
    Array(ArrayElemTypeDef),
    /// `MAP(k, v)` / `MAP<k, v>`: its text
    Map(String),
    /// MySQL `ENUM('a', 'b')`
    Enum(Vec<EnumMember>, Option<u8>),
    /// `STRUCT<...>`: its text
    Struct(String),
    HugeInt,
    UHugeInt,
    UTinyInt,
    USmallInt,
    UBigInt,
    Signed,
    SignedInteger,
    Unsigned,
    UnsignedInteger,
    AnyType,
    TsVector,
    TsQuery,
    /// `POINT`, `POLYGON`, ...: its text
    GeometricType(String),
    VarBit(Option<u64>),
    Regclass,
    /// Any other type: its text
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CharacterLength {
    /// A length, and its unit as written (`CHARACTERS`, `OCTETS`)
    IntegerLength { length: u64, unit: Option<String> },
    /// `MAX`
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExactNumberInfo {
    None,
    Precision(u64),
    PrecisionAndScale(u64, i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimezoneInfo {
    None,
    WithTimeZone,
    WithoutTimeZone,
    /// `TIMESTAMPTZ`
    Tz,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayElemTypeDef {
    /// `ARRAY`
    None,
    /// `ARRAY<INT>`
    AngleBracket(Box<DataType>),
    /// `INT[]`, `INT[3]`
    SquareBracket(Box<DataType>, Option<u64>),
    /// `Array(Int64)`
    Parenthesis(Box<DataType>),
    /// `INT ARRAY`, `INT ARRAY[3]`
    Qualified(Box<DataType>, Option<u64>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EnumMember {
    Name(String),
    NamedValue(String, Expr),
}
