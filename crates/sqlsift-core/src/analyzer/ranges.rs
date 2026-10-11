//! E0017: a literal assigned to a column that can't hold it
//!
//! `INSERT ... VALUES` and `UPDATE ... SET` with a string longer than a
//! `VARCHAR(n)` / `CHAR(n)` column, an integer outside the range of an integer
//! column, or a number with more integer digits than a `DECIMAL(p, s)` column
//! allows. Only literals are checked, and only for the dialects that reject the
//! value instead of truncating it (PostgreSQL, Redshift, MySQL in its default
//! strict mode). Comparisons such as `WHERE age = 99999` are valid and not checked.

use sqlparser::ast::{Expr, UnaryOperator, Value, ValueWithSpan};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{ColumnDef, QualifiedName};
use crate::types::SqlType;

use super::resolver::Resolver;

impl Resolver<'_> {
    /// Report `value` if it is a literal that `column` of `table` can't hold.
    /// `fallback` locates the diagnostic when the literal has no location.
    pub(super) fn check_value_range(
        &mut self,
        table: &QualifiedName,
        column: &ColumnDef,
        value: &Expr,
        fallback: Option<Span>,
    ) {
        if !self.dialect.rejects_out_of_range_values() {
            return;
        }
        let Some(problem) = out_of_range(self.dialect, &column.data_type, value) else {
            return;
        };
        let type_name = column.data_type.display_name();
        let target = format!("{table}.{}", column.name);
        let (message, help) = match problem {
            Problem::TooLong(length) => (
                format!(
                    "Value is too long for column '{target}' of type {type_name}: \
                     {length} characters"
                ),
                "Shorten the value or widen the column",
            ),
            Problem::OutOfRange(literal) => (
                format!(
                    "Value {literal} is out of range for column '{target}' of type {type_name}"
                ),
                "Use a value the column type can hold or a wider column type",
            ),
        };
        let span = super::type_check::located_span(value).or(fallback);
        let mut diag = Diagnostic::error(DiagnosticKind::ValueOutOfRange, message).with_help(help);
        diag.span = span;
        self.diagnostics.push(diag);
    }
}

enum Problem {
    /// A string with this many characters (trailing spaces not counted)
    TooLong(usize),
    /// A number, as written
    OutOfRange(String),
}

fn out_of_range(dialect: SqlDialect, column_type: &SqlType, value: &Expr) -> Option<Problem> {
    match (column_type, value) {
        (
            SqlType::Char {
                length: Some(limit),
            }
            | SqlType::Varchar {
                length: Some(limit),
            },
            Expr::Value(ValueWithSpan {
                value: Value::SingleQuotedString(s),
                ..
            }),
        ) => {
            // Excess trailing spaces are truncated silently
            let length = s.trim_end_matches(' ').chars().count();
            (u64::try_from(length).ok()? > *limit).then_some(Problem::TooLong(length))
        }
        (
            SqlType::Char { .. } | SqlType::Varchar { .. },
            Expr::Value(ValueWithSpan {
                value: Value::DoubleQuotedString(s),
                ..
            }),
        ) if dialect == SqlDialect::MySQL => out_of_range(
            dialect,
            column_type,
            &Expr::value(Value::SingleQuotedString(s.clone())),
        ),
        _ => {
            let literal = signed_number(value)?;
            let fits = if column_type.is_integer() {
                // Literals with a fraction or exponent are rounded first: skipped
                if !literal.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    return None;
                }
                let n: i128 = literal.parse().ok()?;
                let (min, max) = integer_range(dialect, column_type)?;
                (min..=max).contains(&n)
            } else if let SqlType::Decimal {
                precision: Some(precision),
                scale,
            } = column_type
            {
                if literal.contains(['e', 'E']) {
                    return None;
                }
                let integer_part = literal.trim_start_matches('-').split('.').next()?;
                let digits = integer_part.trim_start_matches('0').len();
                // The value is rounded to the scale first, which can only add an
                // integer digit: counting the digits as written never over-reports
                u64::try_from(digits).ok()? <= precision.saturating_sub(scale.unwrap_or(0))
            } else {
                return None;
            };
            (!fits).then_some(Problem::OutOfRange(literal))
        }
    }
}

/// A number literal, with its sign if negated (`-5`, `- 5`)
fn signed_number(value: &Expr) -> Option<String> {
    match value {
        Expr::Value(ValueWithSpan {
            value: Value::Number(n, _),
            ..
        }) => Some(n.clone()),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match expr.as_ref() {
            Expr::Value(ValueWithSpan {
                value: Value::Number(n, _),
                ..
            }) => Some(format!("-{n}")),
            _ => None,
        },
        Expr::Nested(inner) => signed_number(inner),
        _ => None,
    }
}

/// The values an integer column accepts. MySQL integer types may be UNSIGNED,
/// which the catalog doesn't record: they accept the signed minimum to the
/// unsigned maximum here.
fn integer_range(dialect: SqlDialect, column_type: &SqlType) -> Option<(i128, i128)> {
    let bits = match column_type {
        SqlType::TinyInt => 8,
        SqlType::SmallInt => 16,
        SqlType::MediumInt => 24,
        SqlType::Integer => 32,
        SqlType::BigInt => 64,
        _ => return None,
    };
    // PostgreSQL and Redshift have no 1- or 3-byte integers
    if bits % 16 != 0 && dialect != SqlDialect::MySQL {
        return None;
    }
    let min = -(1i128 << (bits - 1));
    let max = if dialect == SqlDialect::MySQL {
        (1i128 << bits) - 1
    } else {
        (1i128 << (bits - 1)) - 1
    };
    Some((min, max))
}
