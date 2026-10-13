//! SQL text of the tree, as sqlparser prints its own (on one line).

use std::fmt::{self, Display, Formatter, Write as _};

use crate::{
    AccessExpr, AlterTable, Array, ArrayElemTypeDef, Assignment, AssignmentTarget, CaseWhen,
    CastKind, CeilFloorKind, CharacterLength, ConflictTarget, CreateTable, CreateView, Cte,
    CteAsMaterialized, DataType, Delete, Distinct, DollarQuotedString, DuplicateTreatment,
    EnumMember, ExactNumberInfo, ExceptSelectItem, ExcludeSelectItem, Expr, ExprWithAlias,
    ExtractSyntax, Fetch, FromTable, Function, FunctionArg, FunctionArgExpr, FunctionArgOperator,
    FunctionArgumentClause, FunctionArgumentList, FunctionArguments, GroupByExpr,
    GroupByWithModifier, Ident, IlikeSelectItem, Insert, Interval, Join, JoinConstraint,
    JoinOperator, JsonPath, JsonPathElem, LimitClause, NamedWindowDefinition, NamedWindowExpr,
    NullTreatment, ObjectName, ObjectNamePart, ObjectNamePartFunction, Offset, OffsetRows,
    OnConflict, OnConflictAction, OnInsert, OrderBy, OrderByExpr, OrderByKind, OrderByOptions,
    OrderBySort, Query, RenameSelectItem, ReplaceSelectItem, Select, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, SetOperator, SetQuantifier, Statement, Subscript,
    Table, TableAlias, TableAliasColumnDef, TableFactor, TableObject, TableWithJoins, TimezoneInfo,
    TrimWhereField, TypedString, UnaryOperator, Update, UpdateTableFromKind, Value, ValueWithSpan,
    Values, WildcardAdditionalOptions, WindowFrameBound, WindowFrameUnits, WindowSpec, WindowType,
    With,
};

/// Items separated by `sep`.
struct Separated<'a, T>(&'a [T], &'static str);

impl<T: Display> Display for Separated<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(self.1)?;
            }
            item.fmt(f)?;
        }
        Ok(())
    }
}

fn comma<T: Display>(items: &[T]) -> Separated<'_, T> {
    Separated(items, ", ")
}

/// A string in quotes, with unescaped quotes doubled (quotes already doubled or
/// escaped with a backslash are kept), as sqlparser prints string literals.
struct EscapeQuoted<'a>(&'a str, char);

impl Display for EscapeQuoted<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (string, quote) = (self.0, self.1);
        let mut previous = char::default();
        let mut start = 0;
        let mut chars = string.char_indices().peekable();
        while let Some(&(index, ch)) = chars.peek() {
            if ch == quote {
                if previous == '\\' {
                    chars.next();
                    continue;
                }
                chars.next();
                if chars.peek().is_some_and(|&(_, c)| c == quote) {
                    chars.next();
                } else {
                    f.write_str(&string[start..index + ch.len_utf8()])?;
                    start = index;
                }
            } else {
                chars.next();
            }
            previous = ch;
        }
        f.write_str(&string[start..])
    }
}

impl Display for Ident {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.quote_style {
            Some(q @ ('"' | '\'' | '`')) => write!(f, "{q}{}{q}", EscapeQuoted(&self.value, q)),
            Some('[') => write!(f, "[{}]", self.value),
            Some(q) => write!(f, "{q}{}{q}", self.value),
            None => f.write_str(&self.value),
        }
    }
}

impl Display for ObjectName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Separated(&self.0, ".").fmt(f)
    }
}

impl Display for ObjectNamePart {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ObjectNamePart::Identifier(ident) => ident.fmt(f),
            ObjectNamePart::Function(function) => function.fmt(f),
        }
    }
}

impl Display for ObjectNamePartFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.name, comma(&self.args))
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Value::Number(v, long) => write!(f, "{v}{}", if *long { "L" } else { "" }),
            Value::DoubleQuotedString(v) => write!(f, "\"{}\"", EscapeQuoted(v, '"')),
            Value::SingleQuotedString(v) => write!(f, "'{}'", EscapeQuoted(v, '\'')),
            Value::TripleSingleQuotedString(v) => write!(f, "'''{v}'''"),
            Value::TripleDoubleQuotedString(v) => write!(f, r#""""{v}""""#),
            Value::DollarQuotedString(v) => v.fmt(f),
            Value::EscapedStringLiteral(v) => {
                f.write_str("E'")?;
                for c in v.chars() {
                    match c {
                        '\'' => f.write_str(r"\'")?,
                        '\\' => f.write_str(r"\\")?,
                        '\n' => f.write_str(r"\n")?,
                        '\t' => f.write_str(r"\t")?,
                        '\r' => f.write_str(r"\r")?,
                        _ => f.write_char(c)?,
                    }
                }
                f.write_str("'")
            }
            Value::UnicodeStringLiteral(v) => {
                f.write_str("U&'")?;
                for c in v.chars() {
                    match c {
                        '\'' => f.write_str("''")?,
                        '\\' => f.write_str(r"\\")?,
                        c if c.is_ascii() => f.write_char(c)?,
                        c if u32::from(c) <= 0xFFFF => write!(f, "\\{:04X}", u32::from(c))?,
                        c => write!(f, "\\+{:06X}", u32::from(c))?,
                    }
                }
                f.write_str("'")
            }
            Value::NationalStringLiteral(v) => write!(f, "N'{}'", EscapeQuoted(v, '\'')),
            Value::QuoteDelimitedStringLiteral(v) => f.write_str(v),
            Value::NationalQuoteDelimitedStringLiteral(v) => write!(f, "N{v}"),
            Value::HexStringLiteral(v) => write!(f, "X'{v}'"),
            Value::Boolean(v) => write!(f, "{v}"),
            Value::SingleQuotedByteStringLiteral(v) => write!(f, "B'{v}'"),
            Value::DoubleQuotedByteStringLiteral(v) => write!(f, "B\"{v}\""),
            Value::TripleSingleQuotedByteStringLiteral(v) => write!(f, "B'''{v}'''"),
            Value::TripleDoubleQuotedByteStringLiteral(v) => write!(f, r#"B"""{v}""""#),
            Value::SingleQuotedRawStringLiteral(v) => write!(f, "R'{v}'"),
            Value::DoubleQuotedRawStringLiteral(v) => write!(f, "R\"{v}\""),
            Value::TripleSingleQuotedRawStringLiteral(v) => write!(f, "R'''{v}'''"),
            Value::TripleDoubleQuotedRawStringLiteral(v) => write!(f, r#"R"""{v}""""#),
            Value::Null => f.write_str("NULL"),
            Value::Placeholder(v) => f.write_str(v),
        }
    }
}

impl Display for DollarQuotedString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.tag {
            Some(tag) => write!(f, "${tag}${}${tag}$", self.value),
            None => write!(f, "$${}$$", self.value),
        }
    }
}

impl Display for ValueWithSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

fn with_length(f: &mut Formatter<'_>, name: &str, len: Option<u64>, unsigned: bool) -> fmt::Result {
    f.write_str(name)?;
    if let Some(len) = len {
        write!(f, "({len})")?;
    }
    if unsigned {
        f.write_str(" UNSIGNED")?;
    }
    Ok(())
}

fn character(f: &mut Formatter<'_>, name: &str, size: Option<&CharacterLength>) -> fmt::Result {
    f.write_str(name)?;
    if let Some(size) = size {
        write!(f, "({size})")?;
    }
    Ok(())
}

fn with_time_zone(
    f: &mut Formatter<'_>,
    name: &str,
    precision: Option<u64>,
    tz: TimezoneInfo,
) -> fmt::Result {
    f.write_str(name)?;
    if tz == TimezoneInfo::Tz {
        f.write_str("TZ")?;
    }
    if let Some(precision) = precision {
        write!(f, "({precision})")?;
    }
    match tz {
        TimezoneInfo::WithTimeZone => f.write_str(" WITH TIME ZONE"),
        TimezoneInfo::WithoutTimeZone => f.write_str(" WITHOUT TIME ZONE"),
        TimezoneInfo::None | TimezoneInfo::Tz => Ok(()),
    }
}

impl Display for CharacterLength {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CharacterLength::IntegerLength { length, unit } => {
                write!(f, "{length}")?;
                if let Some(unit) = unit {
                    write!(f, " {unit}")?;
                }
                Ok(())
            }
            CharacterLength::Max => f.write_str("MAX"),
        }
    }
}

impl Display for ExactNumberInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ExactNumberInfo::None => Ok(()),
            ExactNumberInfo::Precision(p) => write!(f, "({p})"),
            ExactNumberInfo::PrecisionAndScale(p, s) => write!(f, "({p},{s})"),
        }
    }
}

impl Display for DataType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Character(size) => character(f, "CHARACTER", size.as_ref()),
            DataType::Char(size) => character(f, "CHAR", size.as_ref()),
            DataType::CharacterVarying(size) => character(f, "CHARACTER VARYING", size.as_ref()),
            DataType::Varchar(size) => character(f, "VARCHAR", size.as_ref()),
            DataType::Nvarchar(size) => character(f, "NVARCHAR", size.as_ref()),
            DataType::Uuid => f.write_str("UUID"),
            DataType::Clob(size) => with_length(f, "CLOB", *size, false),
            DataType::Binary(size) => with_length(f, "BINARY", *size, false),
            DataType::Varbinary(size) => with_length(f, "VARBINARY", *size, false),
            DataType::Blob(size) => with_length(f, "BLOB", *size, false),
            DataType::TinyBlob => f.write_str("TINYBLOB"),
            DataType::MediumBlob => f.write_str("MEDIUMBLOB"),
            DataType::LongBlob => f.write_str("LONGBLOB"),
            DataType::Bytes(size) => with_length(f, "BYTES", *size, false),
            DataType::Numeric(info) => write!(f, "NUMERIC{info}"),
            DataType::Decimal(info) => write!(f, "DECIMAL{info}"),
            DataType::DecimalUnsigned(info) => write!(f, "DECIMAL{info} UNSIGNED"),
            DataType::Dec(info) => write!(f, "DEC{info}"),
            DataType::DecUnsigned(info) => write!(f, "DEC{info} UNSIGNED"),
            DataType::BigNumeric(info) => write!(f, "BIGNUMERIC{info}"),
            DataType::BigDecimal(info) => write!(f, "BIGDECIMAL{info}"),
            DataType::Float(info) => write!(f, "FLOAT{info}"),
            DataType::FloatUnsigned(info) => write!(f, "FLOAT{info} UNSIGNED"),
            DataType::TinyInt(n) => with_length(f, "TINYINT", *n, false),
            DataType::TinyIntUnsigned(n) => with_length(f, "TINYINT", *n, true),
            DataType::Int2(n) => with_length(f, "INT2", *n, false),
            DataType::Int2Unsigned(n) => with_length(f, "INT2", *n, true),
            DataType::SmallInt(n) => with_length(f, "SMALLINT", *n, false),
            DataType::SmallIntUnsigned(n) => with_length(f, "SMALLINT", *n, true),
            DataType::MediumInt(n) => with_length(f, "MEDIUMINT", *n, false),
            DataType::MediumIntUnsigned(n) => with_length(f, "MEDIUMINT", *n, true),
            DataType::Int(n) => with_length(f, "INT", *n, false),
            DataType::IntUnsigned(n) => with_length(f, "INT", *n, true),
            DataType::Int4(n) => with_length(f, "INT4", *n, false),
            DataType::Int8(n) => with_length(f, "INT8", *n, false),
            DataType::Int64 => f.write_str("INT64"),
            DataType::Int4Unsigned(n) => with_length(f, "INT4", *n, true),
            DataType::Integer(n) => with_length(f, "INTEGER", *n, false),
            DataType::IntegerUnsigned(n) => with_length(f, "INTEGER", *n, true),
            DataType::BigInt(n) => with_length(f, "BIGINT", *n, false),
            DataType::BigIntUnsigned(n) => with_length(f, "BIGINT", *n, true),
            DataType::Int8Unsigned(n) => with_length(f, "INT8", *n, true),
            DataType::HugeInt => f.write_str("HUGEINT"),
            DataType::UHugeInt => f.write_str("UHUGEINT"),
            DataType::UTinyInt => f.write_str("UTINYINT"),
            DataType::USmallInt => f.write_str("USMALLINT"),
            DataType::UBigInt => f.write_str("UBIGINT"),
            DataType::Signed => f.write_str("SIGNED"),
            DataType::SignedInteger => f.write_str("SIGNED INTEGER"),
            DataType::Unsigned => f.write_str("UNSIGNED"),
            DataType::UnsignedInteger => f.write_str("UNSIGNED INTEGER"),
            DataType::Float4 => f.write_str("FLOAT4"),
            DataType::Float32 => f.write_str("Float32"),
            DataType::Float64 => f.write_str("FLOAT64"),
            DataType::Real => f.write_str("REAL"),
            DataType::RealUnsigned => f.write_str("REAL UNSIGNED"),
            DataType::Float8 => f.write_str("FLOAT8"),
            DataType::Double(info) => write!(f, "DOUBLE{info}"),
            DataType::DoubleUnsigned(info) => write!(f, "DOUBLE{info} UNSIGNED"),
            DataType::DoublePrecision => f.write_str("DOUBLE PRECISION"),
            DataType::DoublePrecisionUnsigned => f.write_str("DOUBLE PRECISION UNSIGNED"),
            DataType::Bool => f.write_str("BOOL"),
            DataType::Boolean => f.write_str("BOOLEAN"),
            DataType::Date => f.write_str("DATE"),
            DataType::Time(precision, tz) => with_time_zone(f, "TIME", *precision, *tz),
            DataType::Datetime(precision) => with_length(f, "DATETIME", *precision, false),
            DataType::Timestamp(precision, tz) => with_time_zone(f, "TIMESTAMP", *precision, *tz),
            DataType::TimestampNtz(precision) => with_length(f, "TIMESTAMP_NTZ", *precision, false),
            DataType::JSON => f.write_str("JSON"),
            DataType::JSONB => f.write_str("JSONB"),
            DataType::Regclass => f.write_str("REGCLASS"),
            DataType::Text => f.write_str("TEXT"),
            DataType::TinyText => f.write_str("TINYTEXT"),
            DataType::MediumText => f.write_str("MEDIUMTEXT"),
            DataType::LongText => f.write_str("LONGTEXT"),
            DataType::String(size) => with_length(f, "STRING", *size, false),
            DataType::Bytea => f.write_str("BYTEA"),
            DataType::VarBit(size) => with_length(f, "VARBIT", *size, false),
            DataType::AnyType => f.write_str("ANY TYPE"),
            DataType::TsVector => f.write_str("TSVECTOR"),
            DataType::TsQuery => f.write_str("TSQUERY"),
            DataType::Array(element) => match element {
                ArrayElemTypeDef::None => f.write_str("ARRAY"),
                ArrayElemTypeDef::SquareBracket(t, None) => write!(f, "{t}[]"),
                ArrayElemTypeDef::SquareBracket(t, Some(size)) => write!(f, "{t}[{size}]"),
                ArrayElemTypeDef::AngleBracket(t) => write!(f, "ARRAY<{t}>"),
                ArrayElemTypeDef::Parenthesis(t) => write!(f, "Array({t})"),
                ArrayElemTypeDef::Qualified(t, None) => write!(f, "{t} ARRAY"),
                ArrayElemTypeDef::Qualified(t, Some(size)) => write!(f, "{t} ARRAY[{size}]"),
            },
            DataType::Custom(name, modifiers) => {
                if modifiers.is_empty() {
                    write!(f, "{name}")
                } else {
                    write!(f, "{name}({})", modifiers.join(", "))
                }
            }
            DataType::Enum(members, bits) => {
                f.write_str("ENUM")?;
                if let Some(bits) = bits {
                    write!(f, "{bits}")?;
                }
                f.write_str("(")?;
                for (i, member) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    match member {
                        EnumMember::Name(name) => write!(f, "'{}'", EscapeQuoted(name, '\''))?,
                        EnumMember::NamedValue(name, value) => {
                            write!(f, "'{}' = {value}", EscapeQuoted(name, '\''))?;
                        }
                    }
                }
                f.write_str(")")
            }
            DataType::GeometricType(text)
            | DataType::Interval(text)
            | DataType::Map(text)
            | DataType::Struct(text)
            | DataType::Other(text) => f.write_str(text),
        }
    }
}

impl Display for Expr {
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let not = |negated: bool| if negated { "NOT " } else { "" };
        match self {
            Expr::Identifier(ident) => ident.fmt(f),
            Expr::Wildcard(_) => f.write_str("*"),
            Expr::QualifiedWildcard(prefix, _) => write!(f, "{prefix}.*"),
            Expr::CompoundIdentifier(idents) => Separated(idents, ".").fmt(f),
            Expr::CompoundFieldAccess { root, access_chain } => {
                root.fmt(f)?;
                for access in access_chain {
                    access.fmt(f)?;
                }
                Ok(())
            }
            Expr::JsonAccess { value, path } => write!(f, "{value}{path}"),
            Expr::IsTrue(e) => write!(f, "{e} IS TRUE"),
            Expr::IsNotTrue(e) => write!(f, "{e} IS NOT TRUE"),
            Expr::IsFalse(e) => write!(f, "{e} IS FALSE"),
            Expr::IsNotFalse(e) => write!(f, "{e} IS NOT FALSE"),
            Expr::IsNull(e) => write!(f, "{e} IS NULL"),
            Expr::IsNotNull(e) => write!(f, "{e} IS NOT NULL"),
            Expr::IsUnknown(e) => write!(f, "{e} IS UNKNOWN"),
            Expr::IsNotUnknown(e) => write!(f, "{e} IS NOT UNKNOWN"),
            Expr::IsDistinctFrom(a, b) => write!(f, "{a} IS DISTINCT FROM {b}"),
            Expr::IsNotDistinctFrom(a, b) => write!(f, "{a} IS NOT DISTINCT FROM {b}"),
            Expr::InList {
                expr,
                list,
                negated,
            } => write!(f, "{expr} {}IN ({})", not(*negated), comma(list)),
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => write!(f, "{expr} {}IN ({subquery})", not(*negated)),
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => write!(f, "{expr} {}BETWEEN {low} AND {high}", not(*negated)),
            Expr::BinaryOp { left, op, right } => write!(f, "{left} {op} {right}"),
            Expr::Like {
                negated,
                any,
                expr,
                pattern,
                escape_char,
            } => {
                let any = if *any { "ANY " } else { "" };
                write!(f, "{expr} {}LIKE {any}{pattern}", not(*negated))?;
                if let Some(escape) = escape_char {
                    write!(f, " ESCAPE {escape}")?;
                }
                Ok(())
            }
            Expr::ILike {
                negated,
                any,
                expr,
                pattern,
                escape_char,
            } => {
                if let Some(escape) = escape_char {
                    // sqlparser prints no space after ANY here
                    let any = if *any { "ANY" } else { "" };
                    write!(
                        f,
                        "{expr} {}ILIKE {any}{pattern} ESCAPE {escape}",
                        not(*negated)
                    )
                } else {
                    let any = if *any { "ANY " } else { "" };
                    write!(f, "{expr} {}ILIKE {any}{pattern}", not(*negated))
                }
            }
            Expr::SimilarTo {
                negated,
                expr,
                pattern,
                escape_char,
            } => {
                write!(f, "{expr} {}SIMILAR TO {pattern}", not(*negated))?;
                if let Some(escape) = escape_char {
                    write!(f, " ESCAPE {escape}")?;
                }
                Ok(())
            }
            Expr::RLike {
                negated,
                expr,
                pattern,
                regexp,
            } => write!(
                f,
                "{expr} {}{} {pattern}",
                not(*negated),
                if *regexp { "REGEXP" } else { "RLIKE" }
            ),
            Expr::AnyOp {
                left,
                compare_op,
                right,
                is_some,
            } => {
                let parens = !matches!(right.as_ref(), Expr::Subquery(_));
                write!(
                    f,
                    "{left} {compare_op} {}{}{right}{}",
                    if *is_some { "SOME" } else { "ANY" },
                    if parens { "(" } else { "" },
                    if parens { ")" } else { "" }
                )
            }
            Expr::AllOp {
                left,
                compare_op,
                right,
            } => {
                let parens = !matches!(right.as_ref(), Expr::Subquery(_));
                write!(
                    f,
                    "{left} {compare_op} ALL{}{right}{}",
                    if parens { "(" } else { "" },
                    if parens { ")" } else { "" }
                )
            }
            Expr::UnaryOp { op, expr } => {
                if *op == UnaryOperator::PGPostfixFactorial {
                    write!(f, "{expr}{op}")
                } else if matches!(
                    op,
                    UnaryOperator::Not
                        | UnaryOperator::Hash
                        | UnaryOperator::AtDashAt
                        | UnaryOperator::DoubleAt
                        | UnaryOperator::QuestionDash
                        | UnaryOperator::QuestionPipe
                ) {
                    write!(f, "{op} {expr}")
                } else {
                    write!(f, "{op}{expr}")
                }
            }
            Expr::Cast {
                kind,
                expr,
                data_type,
                format,
            } => {
                let format = format
                    .as_ref()
                    .map(|format| format!(" FORMAT {format}"))
                    .unwrap_or_default();
                match kind {
                    CastKind::Cast => write!(f, "CAST({expr} AS {data_type}{format})"),
                    CastKind::TryCast => write!(f, "TRY_CAST({expr} AS {data_type}{format})"),
                    CastKind::SafeCast => write!(f, "SAFE_CAST({expr} AS {data_type}{format})"),
                    CastKind::DoubleColon => write!(f, "{expr}::{data_type}"),
                }
            }
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => write!(f, "{timestamp} AT TIME ZONE {time_zone}"),
            Expr::Extract {
                field,
                syntax,
                expr,
            } => match syntax {
                ExtractSyntax::From => write!(f, "EXTRACT({field} FROM {expr})"),
                ExtractSyntax::Comma => write!(f, "EXTRACT({field}, {expr})"),
            },
            Expr::Ceil { expr, field } => ceil_floor(f, "CEIL", expr, field),
            Expr::Floor { expr, field } => ceil_floor(f, "FLOOR", expr, field),
            Expr::Position { expr, r#in } => write!(f, "POSITION({expr} IN {in})"),
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                special,
                shorthand,
            } => {
                f.write_str(if *shorthand { "SUBSTR" } else { "SUBSTRING" })?;
                write!(f, "({expr}")?;
                if let Some(from) = substring_from {
                    if *special {
                        write!(f, ", {from}")?;
                    } else {
                        write!(f, " FROM {from}")?;
                    }
                }
                if let Some(length) = substring_for {
                    if *special {
                        write!(f, ", {length}")?;
                    } else {
                        write!(f, " FOR {length}")?;
                    }
                }
                f.write_str(")")
            }
            Expr::Trim {
                trim_where,
                trim_what,
                expr,
                trim_characters,
            } => {
                f.write_str("TRIM(")?;
                if let Some(side) = trim_where {
                    write!(f, "{side} ")?;
                }
                match trim_what {
                    Some(what) => write!(f, "{what} FROM {expr}")?,
                    None => write!(f, "{expr}")?,
                }
                if let Some(characters) = trim_characters {
                    write!(f, ", {}", comma(characters))?;
                }
                f.write_str(")")
            }
            Expr::Overlay {
                expr,
                overlay_what,
                overlay_from,
                overlay_for,
            } => {
                write!(
                    f,
                    "OVERLAY({expr} PLACING {overlay_what} FROM {overlay_from}"
                )?;
                if let Some(length) = overlay_for {
                    write!(f, " FOR {length}")?;
                }
                f.write_str(")")
            }
            Expr::Collate { expr, collation } => write!(f, "{expr} COLLATE {collation}"),
            Expr::Nested(e) => write!(f, "({e})"),
            Expr::Value(v) => v.fmt(f),
            Expr::TypedString(typed) => typed.fmt(f),
            Expr::Function(function) => function.fmt(f),
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                f.write_str("CASE")?;
                if let Some(operand) = operand {
                    write!(f, " {operand}")?;
                }
                for when in conditions {
                    write!(f, " {when}")?;
                }
                if let Some(else_result) = else_result {
                    write!(f, " ELSE {else_result}")?;
                }
                f.write_str(" END")
            }
            Expr::Exists { subquery, negated } => {
                write!(f, "{}EXISTS ({subquery})", not(*negated))
            }
            Expr::Subquery(query) => write!(f, "({query})"),
            Expr::GroupingSets(sets) => {
                f.write_str("GROUPING SETS (")?;
                for (i, set) in sets.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "({})", comma(set))?;
                }
                f.write_str(")")
            }
            Expr::Cube(sets) => grouping_lists(f, "CUBE", sets),
            Expr::Rollup(sets) => grouping_lists(f, "ROLLUP", sets),
            Expr::Tuple(exprs) => write!(f, "({})", comma(exprs)),
            Expr::Array(array) => array.fmt(f),
            Expr::Interval(interval) => interval.fmt(f),
            Expr::Other(other) => f.write_str(&other.text),
        }
    }
}

fn ceil_floor(
    f: &mut Formatter<'_>,
    name: &str,
    expr: &Expr,
    field: &CeilFloorKind,
) -> fmt::Result {
    match field {
        CeilFloorKind::DateTimeField(field) if field.is_empty() => write!(f, "{name}({expr})"),
        CeilFloorKind::DateTimeField(field) => write!(f, "{name}({expr} TO {field})"),
        CeilFloorKind::Scale(scale) => write!(f, "{name}({expr}, {scale})"),
    }
}

fn grouping_lists(f: &mut Formatter<'_>, name: &str, sets: &[Vec<Expr>]) -> fmt::Result {
    write!(f, "{name} (")?;
    for (i, set) in sets.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        if let [single] = set.as_slice() {
            write!(f, "{single}")?;
        } else {
            write!(f, "({})", comma(set))?;
        }
    }
    f.write_str(")")
}

impl Display for AccessExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AccessExpr::Dot(Expr::Value(value)) if matches!(value.value, Value::Number(..)) => {
                write!(f, " . {value}")
            }
            AccessExpr::Dot(expr) => write!(f, ".{expr}"),
            AccessExpr::Subscript(subscript) => write!(f, "[{subscript}]"),
        }
    }
}

impl Display for Subscript {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Subscript::Index { index } => index.fmt(f),
            Subscript::Slice {
                lower_bound,
                upper_bound,
                stride,
            } => {
                if let Some(lower) = lower_bound {
                    lower.fmt(f)?;
                }
                f.write_str(":")?;
                if let Some(upper) = upper_bound {
                    upper.fmt(f)?;
                }
                if let Some(stride) = stride {
                    write!(f, ":{stride}")?;
                }
                Ok(())
            }
        }
    }
}

impl Display for JsonPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (i, elem) in self.path.iter().enumerate() {
            match elem {
                JsonPathElem::Dot { key, quoted } => {
                    f.write_str(if i == 0 { ":" } else { "." })?;
                    if *quoted {
                        write!(f, "\"{}\"", EscapeQuoted(key, '"'))?;
                    } else {
                        f.write_str(key)?;
                    }
                }
                JsonPathElem::Bracket { key } => write!(f, "[{key}]")?,
                JsonPathElem::ColonBracket { key } => write!(f, ":[{key}]")?,
            }
        }
        Ok(())
    }
}

impl Display for crate::BinaryOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        use crate::BinaryOperator as B;
        let text = match self {
            B::Plus => "+",
            B::Minus => "-",
            B::Multiply => "*",
            B::Divide => "/",
            B::Modulo => "%",
            B::StringConcat => "||",
            B::Gt => ">",
            B::Lt => "<",
            B::GtEq => ">=",
            B::LtEq => "<=",
            B::Spaceship => "<=>",
            B::Eq => "=",
            B::NotEq => "<>",
            B::And => "AND",
            B::Or => "OR",
            B::Xor => "XOR",
            B::BitwiseOr => "|",
            B::BitwiseAnd => "&",
            B::BitwiseXor | B::PGExp => "^",
            B::DuckIntegerDivide => "//",
            B::MyIntegerDivide => "DIV",
            B::Match => "MATCH",
            B::Regexp => "REGEXP",
            B::Glob => "GLOB",
            B::Custom(s) => s,
            B::PGBitwiseXor => "#",
            B::PGBitwiseShiftLeft => "<<",
            B::PGBitwiseShiftRight => ">>",
            B::PGOverlap => "&&",
            B::PGRegexMatch => "~",
            B::PGRegexIMatch => "~*",
            B::PGRegexNotMatch => "!~",
            B::PGRegexNotIMatch => "!~*",
            B::PGLikeMatch => "~~",
            B::PGILikeMatch => "~~*",
            B::PGNotLikeMatch => "!~~",
            B::PGNotILikeMatch => "!~~*",
            B::PGStartsWith => "^@",
            B::Arrow => "->",
            B::LongArrow => "->>",
            B::HashArrow => "#>",
            B::HashLongArrow => "#>>",
            B::AtAt => "@@",
            B::AtArrow => "@>",
            B::ArrowAt => "<@",
            B::HashMinus => "#-",
            B::AtQuestion => "@?",
            B::Question => "?",
            B::QuestionAnd => "?&",
            B::QuestionPipe => "?|",
            B::PGCustomBinaryOperator(idents) => {
                return write!(f, "OPERATOR({})", Separated(idents, "."));
            }
            B::Overlaps => "OVERLAPS",
            B::DoubleHash => "##",
            B::LtDashGt => "<->",
            B::AndLt => "&<",
            B::AndGt => "&>",
            B::LtLtPipe => "<<|",
            B::PipeGtGt => "|>>",
            B::AndLtPipe => "&<|",
            B::PipeAndGt => "|&>",
            B::LtCaret => "<^",
            B::GtCaret => ">^",
            B::QuestionHash => "?#",
            B::QuestionDash => "?-",
            B::QuestionDashPipe => "?-|",
            B::QuestionDoublePipe => "?||",
            B::At => "@",
            B::TildeEq => "~=",
            B::Assignment => ":=",
        };
        f.write_str(text)
    }
}

impl Display for UnaryOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOperator::AtDashAt => "@-@",
            UnaryOperator::BangNot | UnaryOperator::PGPostfixFactorial => "!",
            UnaryOperator::BitwiseNot => "~",
            UnaryOperator::DoubleAt => "@@",
            UnaryOperator::Hash => "#",
            UnaryOperator::Minus => "-",
            UnaryOperator::Not => "NOT",
            UnaryOperator::PGAbs => "@",
            UnaryOperator::PGCubeRoot => "||/",
            UnaryOperator::PGPrefixFactorial => "!!",
            UnaryOperator::PGSquareRoot => "|/",
            UnaryOperator::Plus => "+",
            UnaryOperator::QuestionDash => "?-",
            UnaryOperator::QuestionPipe => "?|",
        })
    }
}

impl Display for TrimWhereField {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TrimWhereField::Both => "BOTH",
            TrimWhereField::Leading => "LEADING",
            TrimWhereField::Trailing => "TRAILING",
        })
    }
}

impl Display for TypedString {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.uses_odbc_syntax {
            let prefix = match self.data_type {
                DataType::Date => "d",
                DataType::Time(..) => "t",
                DataType::Timestamp(..) => "ts",
                _ => "?",
            };
            write!(f, "{{{prefix} {}}}", self.value)
        } else {
            write!(f, "{} {}", self.data_type, self.value)
        }
    }
}

impl Display for CaseWhen {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "WHEN {} THEN {}", self.condition, self.result)
    }
}

impl Display for Array {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}]",
            if self.named { "ARRAY" } else { "" },
            comma(&self.elem)
        )
    }
}

impl Display for Interval {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "INTERVAL {}", self.value)?;
        if let Some(qualifier) = &self.qualifier {
            write!(f, " {qualifier}")?;
        }
        Ok(())
    }
}

impl Display for Function {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.uses_odbc_syntax {
            f.write_str("{fn ")?;
        }
        write!(f, "{}{}{}", self.name, self.parameters, self.args)?;
        if !self.within_group.is_empty() {
            write!(f, " WITHIN GROUP (ORDER BY {})", comma(&self.within_group))?;
        }
        if let Some(filter) = &self.filter {
            write!(f, " FILTER (WHERE {filter})")?;
        }
        if let Some(null_treatment) = &self.null_treatment {
            write!(f, " {null_treatment}")?;
        }
        if let Some(over) = &self.over {
            write!(f, " OVER {over}")?;
        }
        if self.uses_odbc_syntax {
            f.write_str("}")?;
        }
        Ok(())
    }
}

impl Display for FunctionArguments {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FunctionArguments::None => Ok(()),
            FunctionArguments::Subquery(query) => write!(f, "({query})"),
            FunctionArguments::List(list) => write!(f, "({list})"),
        }
    }
}

impl Display for FunctionArgumentList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(duplicate_treatment) = &self.duplicate_treatment {
            write!(f, "{duplicate_treatment} ")?;
        }
        comma(&self.args).fmt(f)?;
        if !self.clauses.is_empty() {
            if !self.args.is_empty() {
                f.write_str(" ")?;
            }
            Separated(&self.clauses, " ").fmt(f)?;
        }
        Ok(())
    }
}

impl Display for FunctionArgumentClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FunctionArgumentClause::IgnoreOrRespectNulls(null_treatment) => null_treatment.fmt(f),
            FunctionArgumentClause::OrderBy(order_by) => {
                write!(f, "ORDER BY {}", comma(order_by))
            }
            FunctionArgumentClause::Limit(limit) => write!(f, "LIMIT {limit}"),
            FunctionArgumentClause::Separator(separator) => write!(f, "SEPARATOR {separator}"),
            FunctionArgumentClause::Other(text, _) => f.write_str(text),
        }
    }
}

impl Display for DuplicateTreatment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DuplicateTreatment::Distinct => "DISTINCT",
            DuplicateTreatment::All => "ALL",
        })
    }
}

impl Display for NullTreatment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NullTreatment::IgnoreNulls => "IGNORE NULLS",
            NullTreatment::RespectNulls => "RESPECT NULLS",
        })
    }
}

impl Display for FunctionArg {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FunctionArg::Named {
                name,
                arg,
                operator,
            } => named_arg(f, name, *operator, arg),
            FunctionArg::ExprNamed {
                name,
                arg,
                operator,
            } => named_arg(f, name, *operator, arg),
            FunctionArg::Unnamed(arg) => arg.fmt(f),
        }
    }
}

fn named_arg(
    f: &mut Formatter<'_>,
    name: &dyn Display,
    operator: FunctionArgOperator,
    arg: &FunctionArgExpr,
) -> fmt::Result {
    if operator == FunctionArgOperator::Space {
        write!(f, "{name} {arg}")
    } else {
        write!(f, "{name} {operator} {arg}")
    }
}

impl Display for FunctionArgOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FunctionArgOperator::Equals => "=",
            FunctionArgOperator::RightArrow => "=>",
            FunctionArgOperator::Assignment => ":=",
            FunctionArgOperator::Colon => ":",
            FunctionArgOperator::Value => "VALUE",
            FunctionArgOperator::Space => " ",
        })
    }
}

impl Display for FunctionArgExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FunctionArgExpr::Expr(expr) => expr.fmt(f),
            FunctionArgExpr::QualifiedWildcard(prefix) => write!(f, "{prefix}.*"),
            FunctionArgExpr::Wildcard => f.write_str("*"),
        }
    }
}

impl Display for WindowType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            WindowType::WindowSpec(spec) => write!(f, "({spec})"),
            WindowType::NamedWindow(name) => name.fmt(f),
        }
    }
}

impl Display for WindowSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if let Some(name) = &self.window_name {
            parts.push(name.to_string());
        }
        if !self.partition_by.is_empty() {
            parts.push(format!("PARTITION BY {}", comma(&self.partition_by)));
        }
        if !self.order_by.is_empty() {
            parts.push(format!("ORDER BY {}", comma(&self.order_by)));
        }
        if let Some(frame) = &self.window_frame {
            parts.push(match &frame.end_bound {
                Some(end) => format!("{} BETWEEN {} AND {end}", frame.units, frame.start_bound),
                None => format!("{} {}", frame.units, frame.start_bound),
            });
        }
        f.write_str(&parts.join(" "))
    }
}

impl Display for WindowFrameUnits {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WindowFrameUnits::Rows => "ROWS",
            WindowFrameUnits::Range => "RANGE",
            WindowFrameUnits::Groups => "GROUPS",
        })
    }
}

impl Display for WindowFrameBound {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            WindowFrameBound::CurrentRow => f.write_str("CURRENT ROW"),
            WindowFrameBound::Preceding(None) => f.write_str("UNBOUNDED PRECEDING"),
            WindowFrameBound::Following(None) => f.write_str("UNBOUNDED FOLLOWING"),
            WindowFrameBound::Preceding(Some(n)) => write!(f, "{n} PRECEDING"),
            WindowFrameBound::Following(Some(n)) => write!(f, "{n} FOLLOWING"),
        }
    }
}

impl Display for OrderByExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.expr, self.options)
    }
}

impl Display for OrderByOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.sort {
            Some(OrderBySort::Asc) => f.write_str(" ASC")?,
            Some(OrderBySort::Desc) => f.write_str(" DESC")?,
            Some(OrderBySort::Using(op)) if op.0.len() > 1 => write!(f, " USING OPERATOR({op})")?,
            Some(OrderBySort::Using(op)) => write!(f, " USING {op}")?,
            None => {}
        }
        match self.nulls_first {
            Some(true) => f.write_str(" NULLS FIRST"),
            Some(false) => f.write_str(" NULLS LAST"),
            None => Ok(()),
        }
    }
}

// Queries

impl Display for Query {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(with) = &self.with {
            write!(f, "{with} ")?;
        }
        self.body.fmt(f)?;
        if let Some(order_by) = &self.order_by {
            write!(f, " {order_by}")?;
        }
        if let Some(limit) = &self.limit_clause {
            limit.fmt(f)?;
        }
        if let Some(fetch) = &self.fetch {
            write!(f, " {fetch}")?;
        }
        for lock in &self.locks {
            write!(f, " {}", lock.text)?;
        }
        Ok(())
    }
}

impl Display for With {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WITH {}{}",
            if self.recursive { "RECURSIVE " } else { "" },
            comma(&self.cte_tables)
        )
    }
}

impl Display for Cte {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.materialized {
            Some(CteAsMaterialized::Materialized) => {
                write!(f, "{} AS MATERIALIZED ({})", self.alias, self.query)?;
            }
            Some(CteAsMaterialized::NotMaterialized) => {
                write!(f, "{} AS NOT MATERIALIZED ({})", self.alias, self.query)?;
            }
            None => write!(f, "{} AS ({})", self.alias, self.query)?,
        }
        if let Some(from) = &self.from {
            write!(f, " FROM {from}")?;
        }
        Ok(())
    }
}

impl Display for SetExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SetExpr::Select(select) => select.fmt(f),
            SetExpr::Query(query) => write!(f, "({query})"),
            SetExpr::Values(values) => values.fmt(f),
            SetExpr::Insert(statement)
            | SetExpr::Update(statement)
            | SetExpr::Delete(statement)
            | SetExpr::Merge(statement) => statement.fmt(f),
            SetExpr::Table(table) => table.fmt(f),
            SetExpr::SetOperation {
                left,
                op,
                set_quantifier,
                right,
            } => {
                write!(f, "{left} {op}")?;
                match set_quantifier {
                    SetQuantifier::None => {}
                    quantifier => write!(f, " {quantifier}")?,
                }
                write!(f, " {right}")
            }
        }
    }
}

impl Display for SetOperator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SetOperator::Union => "UNION",
            SetOperator::Except => "EXCEPT",
            SetOperator::Intersect => "INTERSECT",
            SetOperator::Minus => "MINUS",
        })
    }
}

impl Display for SetQuantifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SetQuantifier::All => "ALL",
            SetQuantifier::Distinct => "DISTINCT",
            SetQuantifier::ByName => "BY NAME",
            SetQuantifier::AllByName => "ALL BY NAME",
            SetQuantifier::DistinctByName => "DISTINCT BY NAME",
            SetQuantifier::None => "",
        })
    }
}

impl Display for Table {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("TABLE")?;
        match (&self.schema_name, &self.table_name) {
            (Some(schema), Some(table)) => write!(f, " {schema}.{table}"),
            (None, Some(table)) => write!(f, " {table}"),
            _ => Ok(()),
        }
    }
}

impl Display for Values {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(if self.value_keyword {
            "VALUE "
        } else {
            "VALUES "
        })?;
        let prefix = if self.explicit_row { "ROW" } else { "" };
        for (i, row) in self.rows.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{prefix}({})", comma(&row.content))?;
        }
        Ok(())
    }
}

impl Display for Select {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("SELECT")?;
        if let Some(distinct) = &self.distinct {
            write!(f, " {distinct}")?;
        }
        if let Some(top) = &self.top {
            write!(f, " TOP {top}")?;
        }
        if let Some(mode) = &self.value_table_mode {
            write!(f, " {mode}")?;
        }
        if !self.projection.is_empty() {
            write!(f, " {}", comma(&self.projection))?;
        }
        if let Some(exclude) = &self.exclude {
            write!(f, " {exclude}")?;
        }
        if let Some(into) = &self.into {
            f.write_str(" INTO ")?;
            if into.temporary {
                f.write_str("TEMPORARY ")?;
            }
            if into.unlogged {
                f.write_str("UNLOGGED ")?;
            }
            if into.table {
                f.write_str("TABLE ")?;
            }
            comma(&into.targets).fmt(f)?;
        }
        if !self.from.is_empty() {
            write!(f, " FROM {}", comma(&self.from))?;
        }
        for view in &self.lateral_views {
            write!(
                f,
                " LATERAL VIEW{} {} {}",
                if view.outer { " OUTER" } else { "" },
                view.lateral_view,
                view.lateral_view_name
            )?;
            if !view.lateral_col_alias.is_empty() {
                write!(f, " AS {}", comma(&view.lateral_col_alias))?;
            }
        }
        if let Some(prewhere) = &self.prewhere {
            write!(f, " PREWHERE {prewhere}")?;
        }
        if let Some(selection) = &self.selection {
            write!(f, " WHERE {selection}")?;
        }
        match &self.group_by {
            GroupByExpr::All(modifiers) => {
                f.write_str(" GROUP BY ALL")?;
                for modifier in modifiers {
                    write!(f, " {modifier}")?;
                }
            }
            GroupByExpr::Expressions(exprs, modifiers) => {
                if !exprs.is_empty() {
                    write!(f, " GROUP BY {}", comma(exprs))?;
                }
                for modifier in modifiers {
                    write!(f, " {modifier}")?;
                }
            }
        }
        if !self.cluster_by.is_empty() {
            write!(f, " CLUSTER BY {}", comma(&self.cluster_by))?;
        }
        if !self.distribute_by.is_empty() {
            write!(f, " DISTRIBUTE BY {}", comma(&self.distribute_by))?;
        }
        if !self.sort_by.is_empty() {
            write!(f, " SORT BY {}", comma(&self.sort_by))?;
        }
        if let Some(having) = &self.having {
            write!(f, " HAVING {having}")?;
        }
        if self.window_before_qualify && !self.named_window.is_empty() {
            write!(f, " WINDOW {}", comma(&self.named_window))?;
        }
        if let Some(qualify) = &self.qualify {
            write!(f, " QUALIFY {qualify}")?;
        }
        if !self.window_before_qualify && !self.named_window.is_empty() {
            write!(f, " WINDOW {}", comma(&self.named_window))?;
        }
        Ok(())
    }
}

impl Display for Distinct {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Distinct::All => f.write_str("ALL"),
            Distinct::Distinct => f.write_str("DISTINCT"),
            Distinct::On(exprs) => write!(f, "DISTINCT ON ({})", comma(exprs)),
        }
    }
}

impl Display for GroupByWithModifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            GroupByWithModifier::Rollup => f.write_str("WITH ROLLUP"),
            GroupByWithModifier::Cube => f.write_str("WITH CUBE"),
            GroupByWithModifier::Totals => f.write_str("WITH TOTALS"),
            GroupByWithModifier::GroupingSets(expr) => expr.fmt(f),
        }
    }
}

impl Display for NamedWindowDefinition {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.1 {
            NamedWindowExpr::NamedWindow(name) => write!(f, "{} AS {name}", self.0),
            NamedWindowExpr::WindowSpec(spec) => write!(f, "{} AS ({spec})", self.0),
        }
    }
}

impl Display for SelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SelectItem::UnnamedExpr(expr) => expr.fmt(f),
            SelectItem::ExprWithAlias { expr, alias } => write!(f, "{expr} AS {alias}"),
            SelectItem::ExprWithAliases { expr, aliases } => {
                write!(f, "{expr} AS ({})", comma(aliases))
            }
            SelectItem::QualifiedWildcard(kind, options) => write!(f, "{kind}.*{options}"),
            SelectItem::Wildcard(options) => write!(f, "*{options}"),
        }
    }
}

impl Display for SelectItemQualifiedWildcardKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SelectItemQualifiedWildcardKind::ObjectName(name) => name.fmt(f),
            SelectItemQualifiedWildcardKind::Expr(expr) => expr.fmt(f),
        }
    }
}

impl Display for WildcardAdditionalOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(ilike) = &self.opt_ilike {
            write!(f, " {ilike}")?;
        }
        if let Some(exclude) = &self.opt_exclude {
            write!(f, " {exclude}")?;
        }
        if let Some(except) = &self.opt_except {
            write!(f, " {except}")?;
        }
        if let Some(replace) = &self.opt_replace {
            write!(f, " {replace}")?;
        }
        if let Some(rename) = &self.opt_rename {
            write!(f, " {rename}")?;
        }
        if let Some(alias) = &self.opt_alias {
            write!(f, " AS {alias}")?;
        }
        Ok(())
    }
}

impl Display for IlikeSelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "ILIKE '{}'", EscapeQuoted(&self.pattern, '\''))
    }
}

impl Display for ExcludeSelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ExcludeSelectItem::Single(column) => write!(f, "EXCLUDE {column}"),
            ExcludeSelectItem::Multiple(columns) => write!(f, "EXCLUDE ({})", comma(columns)),
        }
    }
}

impl Display for ExceptSelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.additional_elements.is_empty() {
            write!(f, "EXCEPT ({})", self.first_element)
        } else {
            write!(
                f,
                "EXCEPT ({}, {})",
                self.first_element,
                comma(&self.additional_elements)
            )
        }
    }
}

impl Display for ReplaceSelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("REPLACE (")?;
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(
                f,
                "{}{}{}",
                item.expr,
                if item.as_keyword { " AS " } else { " " },
                item.column_name
            )?;
        }
        f.write_str(")")
    }
}

impl Display for RenameSelectItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("RENAME ")?;
        match self {
            RenameSelectItem::Single(item) => write!(f, "{} AS {}", item.ident, item.alias),
            RenameSelectItem::Multiple(items) => {
                f.write_str("(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} AS {}", item.ident, item.alias)?;
                }
                f.write_str(")")
            }
        }
    }
}

impl Display for TableWithJoins {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.relation.fmt(f)?;
        for join in &self.joins {
            join.fmt(f)?;
        }
        Ok(())
    }
}

impl Display for Join {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let global = if self.global { " GLOBAL" } else { "" };
        let relation = &self.relation;
        let join = |f: &mut Formatter<'_>, keyword: &str, constraint: &JoinConstraint| {
            let natural = if matches!(constraint, JoinConstraint::Natural) {
                "NATURAL "
            } else {
                ""
            };
            write!(f, "{global} {natural}{keyword} {relation}")?;
            match constraint {
                JoinConstraint::On(expr) => write!(f, " ON {expr}"),
                JoinConstraint::Using(columns) => write!(f, " USING({})", comma(columns)),
                JoinConstraint::Natural | JoinConstraint::None => Ok(()),
            }
        };
        match &self.join_operator {
            JoinOperator::Join(c) => join(f, "JOIN", c),
            JoinOperator::Inner(c) => join(f, "INNER JOIN", c),
            JoinOperator::Left(c) => join(f, "LEFT JOIN", c),
            JoinOperator::LeftOuter(c) => join(f, "LEFT OUTER JOIN", c),
            JoinOperator::Right(c) => join(f, "RIGHT JOIN", c),
            JoinOperator::RightOuter(c) => join(f, "RIGHT OUTER JOIN", c),
            JoinOperator::FullOuter(c) => join(f, "FULL JOIN", c),
            JoinOperator::CrossJoin(c) => join(f, "CROSS JOIN", c),
            JoinOperator::Semi(c) => join(f, "SEMI JOIN", c),
            JoinOperator::LeftSemi(c) => join(f, "LEFT SEMI JOIN", c),
            JoinOperator::RightSemi(c) => join(f, "RIGHT SEMI JOIN", c),
            JoinOperator::Anti(c) => join(f, "ANTI JOIN", c),
            JoinOperator::LeftAnti(c) => join(f, "LEFT ANTI JOIN", c),
            JoinOperator::RightAnti(c) => join(f, "RIGHT ANTI JOIN", c),
            JoinOperator::StraightJoin(c) => join(f, "STRAIGHT_JOIN", c),
            JoinOperator::CrossApply => write!(f, "{global} CROSS APPLY {relation}"),
            JoinOperator::OuterApply => write!(f, "{global} OUTER APPLY {relation}"),
            JoinOperator::AsOf {
                match_condition,
                constraint,
            } => {
                write!(
                    f,
                    "{global} ASOF JOIN {relation} MATCH_CONDITION ({match_condition})"
                )?;
                match constraint {
                    JoinConstraint::On(expr) => write!(f, " ON {expr}"),
                    JoinConstraint::Using(columns) => write!(f, " USING({})", comma(columns)),
                    JoinConstraint::Natural | JoinConstraint::None => Ok(()),
                }
            }
            JoinOperator::ArrayJoin => write!(f, "{global} ARRAY JOIN {relation}"),
            JoinOperator::LeftArrayJoin => write!(f, "{global} LEFT ARRAY JOIN {relation}"),
            JoinOperator::InnerArrayJoin => write!(f, "{global} INNER ARRAY JOIN {relation}"),
        }
    }
}

fn alias(f: &mut Formatter<'_>, alias: Option<&TableAlias>) -> fmt::Result {
    match alias {
        Some(alias) => write!(f, " {alias}"),
        None => Ok(()),
    }
}

impl Display for TableFactor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TableFactor::Table {
                name,
                alias: table_alias,
                args,
                with_ordinality,
                ..
            } => {
                name.fmt(f)?;
                if let Some(args) = args {
                    write!(f, "({})", comma(&args.args))?;
                }
                if *with_ordinality {
                    f.write_str(" WITH ORDINALITY")?;
                }
                alias(f, table_alias.as_ref())
            }
            TableFactor::Derived {
                lateral,
                subquery,
                alias: table_alias,
            } => {
                if *lateral {
                    f.write_str("LATERAL ")?;
                }
                write!(f, "({subquery})")?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::TableFunction {
                expr,
                alias: table_alias,
            } => {
                write!(f, "TABLE({expr})")?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::Function {
                lateral,
                name,
                args,
                with_ordinality,
                alias: table_alias,
            } => {
                if *lateral {
                    f.write_str("LATERAL ")?;
                }
                write!(f, "{name}({})", comma(args))?;
                if *with_ordinality {
                    f.write_str(" WITH ORDINALITY")?;
                }
                alias(f, table_alias.as_ref())
            }
            TableFactor::UNNEST {
                alias: table_alias,
                array_exprs,
                with_offset,
                with_offset_alias,
                with_ordinality,
            } => {
                write!(f, "UNNEST({})", comma(array_exprs))?;
                if *with_ordinality {
                    f.write_str(" WITH ORDINALITY")?;
                }
                alias(f, table_alias.as_ref())?;
                if *with_offset {
                    f.write_str(" WITH OFFSET")?;
                }
                if let Some(offset_alias) = with_offset_alias {
                    write!(f, " AS {offset_alias}")?;
                }
                Ok(())
            }
            TableFactor::JsonTable {
                json_expr,
                json_path,
                alias: table_alias,
                ..
            } => {
                write!(f, "JSON_TABLE({json_expr}, {json_path} COLUMNS(...))")?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::OpenJsonTable {
                json_expr,
                json_path,
                alias: table_alias,
            } => {
                write!(f, "OPENJSON({json_expr}")?;
                if let Some(path) = json_path {
                    write!(f, ", {path}")?;
                }
                f.write_str(")")?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::NestedJoin {
                table_with_joins,
                alias: table_alias,
            } => {
                write!(f, "({table_with_joins})")?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::Pivot {
                table,
                aggregate_functions,
                value_column,
                alias: table_alias,
                ..
            } => {
                write!(
                    f,
                    "{table} PIVOT({} FOR {} IN (...))",
                    comma(aggregate_functions),
                    Separated(value_column, ".")
                )?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::Unpivot {
                table,
                value,
                name,
                columns,
                alias: table_alias,
            } => {
                write!(
                    f,
                    "{table} UNPIVOT({value} FOR {name} IN ({}))",
                    comma(columns)
                )?;
                alias(f, table_alias.as_ref())
            }
            TableFactor::Other(other) => f.write_str(&other.text),
        }
    }
}

impl Display for ExprWithAlias {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.expr.fmt(f)?;
        if let Some(alias) = &self.alias {
            write!(f, " AS {alias}")?;
        }
        Ok(())
    }
}

impl Display for TableAlias {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", if self.explicit { "AS " } else { "" }, self.name)?;
        if !self.columns.is_empty() {
            write!(f, " ({})", comma(&self.columns))?;
        }
        Ok(())
    }
}

impl Display for TableAliasColumnDef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.name.fmt(f)?;
        if let Some(data_type) = &self.data_type {
            write!(f, " {data_type}")?;
        }
        Ok(())
    }
}

impl Display for OrderBy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("ORDER BY")?;
        match &self.kind {
            OrderByKind::All(options) => write!(f, " ALL{options}"),
            OrderByKind::Expressions(exprs) => write!(f, " {}", comma(exprs)),
        }
    }
}

impl Display for LimitClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            } => {
                if let Some(limit) = limit {
                    write!(f, " LIMIT {limit}")?;
                }
                if let Some(offset) = offset {
                    write!(f, " {offset}")?;
                }
                if !limit_by.is_empty() {
                    write!(f, " BY {}", comma(limit_by))?;
                }
                Ok(())
            }
            LimitClause::OffsetCommaLimit { offset, limit } => {
                write!(f, " LIMIT {offset}, {limit}")
            }
        }
    }
}

impl Display for Offset {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "OFFSET {}", self.value)?;
        match self.rows {
            OffsetRows::None => Ok(()),
            OffsetRows::Row => f.write_str(" ROW"),
            OffsetRows::Rows => f.write_str(" ROWS"),
        }
    }
}

impl Display for Fetch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let extension = if self.with_ties { "WITH TIES" } else { "ONLY" };
        match &self.quantity {
            Some(quantity) => {
                let percent = if self.percent { " PERCENT" } else { "" };
                write!(f, "FETCH FIRST {quantity}{percent} ROWS {extension}")
            }
            None => write!(f, "FETCH FIRST ROWS {extension}"),
        }
    }
}

// Statements

impl Display for Statement {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Query(query) => query.fmt(f),
            Statement::Insert(insert) => insert.fmt(f),
            Statement::Update(update) => update.fmt(f),
            Statement::Delete(delete) => delete.fmt(f),
            Statement::Merge(merge) => write!(
                f,
                "MERGE INTO {} USING {} ON {}",
                merge.table, merge.source, merge.on
            ),
            Statement::CreateTable(create) => create.fmt(f),
            Statement::CreateView(view) => view.fmt(f),
            Statement::CreateIndex(index) => write!(
                f,
                "CREATE {}INDEX {}ON {}",
                if index.unique { "UNIQUE " } else { "" },
                index
                    .name
                    .as_ref()
                    .map(|name| format!("{name} "))
                    .unwrap_or_default(),
                index.table_name
            ),
            Statement::CreateType { name, .. } => write!(f, "CREATE TYPE {name}"),
            Statement::CreateTrigger(trigger) => write!(
                f,
                "CREATE TRIGGER {} ON {}",
                trigger.name, trigger.table_name
            ),
            Statement::AlterTable(alter) => alter.fmt(f),
            Statement::AlterType(alter) => write!(f, "ALTER TYPE {}", alter.name),
            Statement::Drop {
                object_type,
                if_exists,
                names,
                ..
            } => write!(
                f,
                "DROP {object_type:?} {}{}",
                if *if_exists { "IF EXISTS " } else { "" },
                comma(names)
            ),
            Statement::Set(_) => f.write_str("SET"),
            Statement::Use(_) => f.write_str("USE"),
            Statement::Declare { stmts } => write!(
                f,
                "DECLARE {}",
                Separated(
                    &stmts
                        .iter()
                        .map(|d| comma(&d.names).to_string())
                        .collect::<Vec<_>>(),
                    "; "
                )
            ),
            Statement::Other(other) => f.write_str(&other.text),
        }
    }
}

impl Display for CreateTable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CREATE {}TABLE {}{}",
            if self.temporary { "TEMPORARY " } else { "" },
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name
        )?;
        if let Some(query) = &self.query {
            write!(f, " AS {query}")?;
        }
        Ok(())
    }
}

impl Display for CreateView {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CREATE {}{}VIEW {} AS {}",
            if self.or_replace { "OR REPLACE " } else { "" },
            if self.materialized {
                "MATERIALIZED "
            } else {
                ""
            },
            self.name,
            self.query
        )
    }
}

impl Display for AlterTable {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "ALTER TABLE {}", self.name)
    }
}

impl Display for Insert {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(if self.replace_into {
            "REPLACE"
        } else {
            "INSERT"
        })?;
        if self.ignore {
            f.write_str(" IGNORE")?;
        }
        if self.into {
            f.write_str(" INTO")?;
        }
        if self.has_table_keyword {
            f.write_str(" TABLE")?;
        }
        write!(f, " {}", self.table)?;
        if let Some(alias) = &self.table_alias {
            write!(f, " AS {}", alias.alias)?;
        }
        if !self.columns.is_empty() {
            write!(f, " ({})", comma(&self.columns))?;
        }
        if let Some(source) = &self.source {
            write!(f, " {source}")?;
        }
        if !self.assignments.is_empty() {
            write!(f, " SET {}", comma(&self.assignments))?;
        } else if self.source.is_none() && self.columns.is_empty() {
            f.write_str(" DEFAULT VALUES")?;
        }
        if let Some(on) = &self.on {
            write!(f, " {on}")?;
        }
        if let Some(returning) = &self.returning {
            write!(f, " RETURNING {}", comma(returning))?;
        }
        Ok(())
    }
}

impl Display for TableObject {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TableObject::TableName(name) => name.fmt(f),
            TableObject::TableFunction(function) => write!(f, "FUNCTION {function}"),
            TableObject::TableQuery(query) => write!(f, "({query})"),
        }
    }
}

impl Display for OnInsert {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            OnInsert::DuplicateKeyUpdate(assignments) => {
                write!(f, "ON DUPLICATE KEY UPDATE {}", comma(assignments))
            }
            OnInsert::OnConflict(on_conflict) => on_conflict.fmt(f),
        }
    }
}

impl Display for OnConflict {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("ON CONFLICT")?;
        match &self.conflict_target {
            Some(ConflictTarget::Columns(columns)) => write!(f, "({})", comma(columns))?,
            Some(ConflictTarget::OnConstraint(name)) => write!(f, " ON CONSTRAINT {name}")?,
            None => {}
        }
        match &self.action {
            OnConflictAction::DoNothing => f.write_str(" DO NOTHING"),
            OnConflictAction::DoUpdate(update) => {
                write!(f, " DO UPDATE SET {}", comma(&update.assignments))?;
                if let Some(selection) = &update.selection {
                    write!(f, " WHERE {selection}")?;
                }
                Ok(())
            }
        }
    }
}

impl Display for Assignment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} = {}", self.target, self.value)
    }
}

impl Display for AssignmentTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AssignmentTarget::ColumnName(name) => name.fmt(f),
            AssignmentTarget::Tuple(names) => write!(f, "({})", comma(names)),
        }
    }
}

impl Display for Update {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("UPDATE ")?;
        if let Some(UpdateTableFromKind::BeforeSet(from)) = &self.from {
            write!(f, "FROM {} ", comma(from))?;
        }
        write!(f, "{}", self.table)?;
        if !self.assignments.is_empty() {
            write!(f, " SET {}", comma(&self.assignments))?;
        }
        if let Some(UpdateTableFromKind::AfterSet(from)) = &self.from {
            write!(f, " FROM {}", comma(from))?;
        }
        if let Some(selection) = &self.selection {
            write!(f, " WHERE {selection}")?;
        }
        if let Some(returning) = &self.returning {
            write!(f, " RETURNING {}", comma(returning))?;
        }
        if !self.order_by.is_empty() {
            write!(f, " ORDER BY {}", comma(&self.order_by))?;
        }
        if let Some(limit) = &self.limit {
            write!(f, " LIMIT {limit}")?;
        }
        Ok(())
    }
}

impl Display for Delete {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("DELETE ")?;
        if !self.tables.is_empty() {
            write!(f, "{} ", comma(&self.tables))?;
        }
        match &self.from {
            FromTable::WithFromKeyword(from) => write!(f, "FROM {}", comma(from))?,
            FromTable::WithoutKeyword(from) => comma(from).fmt(f)?,
        }
        if let Some(using) = &self.using {
            write!(f, " USING {}", comma(using))?;
        }
        if let Some(selection) = &self.selection {
            write!(f, " WHERE {selection}")?;
        }
        if let Some(returning) = &self.returning {
            write!(f, " RETURNING {}", comma(returning))?;
        }
        if !self.order_by.is_empty() {
            write!(f, " ORDER BY {}", comma(&self.order_by))?;
        }
        if let Some(limit) = &self.limit {
            write!(f, " LIMIT {limit}")?;
        }
        Ok(())
    }
}
