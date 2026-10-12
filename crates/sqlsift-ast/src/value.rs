//! Literal values.

use crate::Span;

/// A literal value. String literals keep their kind (quotes, prefix); the text is unescaped
/// as sqlparser does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Value {
    /// A number, as written, and whether it ends in `L` (long, Hive)
    Number(String, bool),
    SingleQuotedString(String),
    /// `$$text$$` / `$tag$text$tag$`
    DollarQuotedString(DollarQuotedString),
    TripleSingleQuotedString(String),
    TripleDoubleQuotedString(String),
    /// `E'...'`
    EscapedStringLiteral(String),
    /// `U&'...'`
    UnicodeStringLiteral(String),
    SingleQuotedByteStringLiteral(String),
    DoubleQuotedByteStringLiteral(String),
    TripleSingleQuotedByteStringLiteral(String),
    TripleDoubleQuotedByteStringLiteral(String),
    SingleQuotedRawStringLiteral(String),
    DoubleQuotedRawStringLiteral(String),
    TripleSingleQuotedRawStringLiteral(String),
    TripleDoubleQuotedRawStringLiteral(String),
    /// `N'...'`
    NationalStringLiteral(String),
    /// Oracle `Q'[...]'`: its text
    QuoteDelimitedStringLiteral(String),
    NationalQuoteDelimitedStringLiteral(String),
    /// `X'...'`
    HexStringLiteral(String),
    DoubleQuotedString(String),
    Boolean(bool),
    Null,
    /// `?`, `$1`, `:name`, `@name`
    Placeholder(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DollarQuotedString {
    pub value: String,
    pub tag: Option<String>,
}

impl Value {
    /// The text of a string literal of any kind (`None` for numbers, booleans, NULL and
    /// placeholders).
    pub fn into_string(self) -> Option<String> {
        match self {
            Value::SingleQuotedString(s)
            | Value::TripleSingleQuotedString(s)
            | Value::TripleDoubleQuotedString(s)
            | Value::EscapedStringLiteral(s)
            | Value::UnicodeStringLiteral(s)
            | Value::SingleQuotedByteStringLiteral(s)
            | Value::DoubleQuotedByteStringLiteral(s)
            | Value::TripleSingleQuotedByteStringLiteral(s)
            | Value::TripleDoubleQuotedByteStringLiteral(s)
            | Value::SingleQuotedRawStringLiteral(s)
            | Value::DoubleQuotedRawStringLiteral(s)
            | Value::TripleSingleQuotedRawStringLiteral(s)
            | Value::TripleDoubleQuotedRawStringLiteral(s)
            | Value::NationalStringLiteral(s)
            | Value::QuoteDelimitedStringLiteral(s)
            | Value::NationalQuoteDelimitedStringLiteral(s)
            | Value::HexStringLiteral(s)
            | Value::DoubleQuotedString(s) => Some(s),
            Value::DollarQuotedString(s) => Some(s.value),
            Value::Number(..) | Value::Boolean(_) | Value::Null | Value::Placeholder(_) => None,
        }
    }

    pub fn with_span(self, span: Span) -> ValueWithSpan {
        ValueWithSpan { value: self, span }
    }
}

/// A value and its location. Values compare equal whatever their locations.
#[derive(Debug, Clone, Eq)]
pub struct ValueWithSpan {
    pub value: Value,
    pub span: Span,
}

impl PartialEq for ValueWithSpan {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl std::hash::Hash for ValueWithSpan {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}
