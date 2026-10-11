//! BigQuery semantics of the resolver (alpha)
//!
//! - `a.b.c` paths: `a` is a table alias or a column, and what follows the column
//!   are STRUCT field names, which sqlsift doesn't model (so never reports)
//! - tables in datasets the schema doesn't define, and wildcard tables
//!   (`` `dataset.events_*` ``), are unknown rather than missing
//! - return types of common BigQuery functions, and the date parts (`DAY`,
//!   `ISOWEEK`, `WEEK(MONDAY)`, ...) their arguments take as bare words
//!
//! sqlparser already splits a backtick-quoted `` `project.dataset.table` `` into its
//! parts in FROM, INSERT, UPDATE, DELETE and CREATE statements, and a three-part
//! name is resolved by its last two parts (dataset and table).

use sqlparser::ast::{Function, FunctionArg, FunctionArgExpr, FunctionArguments, Ident};

use crate::schema::QualifiedName;
use crate::types::SqlType;

use super::resolver::Resolver;
use super::scope::{ColumnLookup, ExpressionType};

impl Resolver<'_> {
    /// Resolve a BigQuery path `a.b[.c...]`: `alias.column[.field...]`,
    /// `column.field[.field...]`, or `table.column` of a table named with its dataset
    pub(super) fn bigquery_path(&mut self, idents: &[Ident]) {
        let [first, second, rest @ ..] = idents else {
            return;
        };
        if self.scope.relation(&first.value).is_some() {
            self.column(Some(first), second);
            return;
        }
        if self.is_value_name(first) {
            // A STRUCT column (or output alias): the rest are its fields
            self.column(None, first);
            return;
        }
        if let Some(third) = rest.first() {
            if self.scope.relation(&second.value).is_some() {
                self.column(Some(second), third);
                return;
            }
        }
        // Neither an alias nor a column: report the unknown alias
        self.column(Some(first), second);
    }

    /// Type of a BigQuery path `a.b[.c...]`: a column's type for `alias.column`,
    /// unknown for STRUCT fields
    pub(super) fn infer_bigquery_path(&self, idents: &[Ident]) -> ExpressionType {
        match idents {
            [table, column] if self.scope.relation(&table.value).is_some() => {
                self.infer_qualified_column(table, column)
            }
            _ => ExpressionType::Unknown,
        }
    }

    /// Whether an unqualified name is a column (or an output column alias) in scope
    fn is_value_name(&self, ident: &Ident) -> bool {
        !matches!(
            self.scope.column(&ident.value, self.dialect),
            ColumnLookup::NotFound
        ) || self.scope.current_ref().is_some_and(|f| {
            f.select_aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&ident.value))
        })
    }

    /// Whether a table missing from the catalog may still exist: a wildcard table
    /// (`events_*`), or a table of a dataset the schema doesn't define (which
    /// sqlsift wasn't given)
    pub(super) fn bigquery_table_may_exist(&self, name: &QualifiedName) -> bool {
        name.name.ends_with('*')
            || name.schema.as_deref().is_some_and(|dataset| {
                !self
                    .catalog
                    .schemas
                    .keys()
                    .any(|s| self.catalog.names_match(s, dataset))
            })
    }

    /// A table of a dataset the schema doesn't define, looked up by its name alone
    /// (schemas often define tables without their dataset)
    pub(super) fn bigquery_unqualified_fallback(
        &self,
        name: &QualifiedName,
    ) -> Option<QualifiedName> {
        if name.schema.is_none() || !self.bigquery_table_may_exist(name) {
            return None;
        }
        let bare = QualifiedName::new(name.name.clone());
        (self.catalog.get_table(&bare).is_some() || self.catalog.get_view(&bare).is_some())
            .then_some(bare)
    }

    /// Return type of a BigQuery function (`name` is uppercase, without a `SAFE.`
    /// prefix), or `None` to use the generic rules
    pub(super) fn bigquery_function_type(
        &self,
        name: &str,
        func: &Function,
    ) -> Option<ExpressionType> {
        let known = |t: SqlType| Some(ExpressionType::Known(t));
        let timestamp = |with_timezone| SqlType::Timestamp {
            precision: None,
            with_timezone,
        };
        let time = SqlType::Time {
            precision: None,
            with_timezone: false,
        };
        let arg = |i: usize| self.infer_nth_arg_type(func, i);
        // The type of an argument when it is known, else `None` (unknown)
        let known_arg = |i: usize| match arg(i) {
            known @ ExpressionType::Known(_) => Some(known),
            _ => Some(ExpressionType::Unknown),
        };
        let array_of = |ty: ExpressionType| match ty {
            ExpressionType::Known(t) => known(SqlType::Array(Box::new(t))),
            ExpressionType::StringLiteral(_) => known(SqlType::Array(Box::new(SqlType::Text))),
            ExpressionType::Unknown => Some(ExpressionType::Unknown),
        };
        // A string function's argument may be STRING or BYTES
        let string_like = |ty: ExpressionType| match ty {
            ExpressionType::Known(SqlType::Bytea) => known(SqlType::Bytea),
            ExpressionType::Known(t) if is_string(&t) => known(SqlType::Text),
            ExpressionType::StringLiteral(_) => known(SqlType::Text),
            _ => Some(ExpressionType::Unknown),
        };

        match name {
            "IF" => match arg(1) {
                known @ ExpressionType::Known(_) => Some(known),
                _ => known_arg(2),
            },
            "ANY_VALUE" | "ARRAY_CONCAT" | "ARRAY_REVERSE" => known_arg(0),

            "COUNTIF"
            | "APPROX_COUNT_DISTINCT"
            | "FARM_FINGERPRINT"
            | "ARRAY_LENGTH"
            | "DATE_DIFF"
            | "DATETIME_DIFF"
            | "TIMESTAMP_DIFF"
            | "TIME_DIFF"
            | "UNIX_DATE"
            | "UNIX_SECONDS"
            | "UNIX_MILLIS"
            | "UNIX_MICROS"
            | "DIV"
            | "BYTE_LENGTH"
            | "INSTR"
            | "INT64" => known(SqlType::BigInt),

            "SAFE_DIVIDE" | "IEEE_DIVIDE" | "RAND" | "FLOAT64" => known(SqlType::DoublePrecision),

            "LOGICAL_OR" | "LOGICAL_AND" | "REGEXP_CONTAINS" | "STARTS_WITH" | "ENDS_WITH"
            | "CONTAINS_SUBSTR" | "IS_NAN" | "IS_INF" | "BOOL" => known(SqlType::Boolean),

            "GENERATE_UUID"
            | "TO_HEX"
            | "TO_BASE64"
            | "TO_BASE32"
            | "FORMAT"
            | "FORMAT_DATE"
            | "FORMAT_DATETIME"
            | "FORMAT_TIMESTAMP"
            | "FORMAT_TIME"
            | "JSON_VALUE"
            | "JSON_EXTRACT_SCALAR"
            | "TO_JSON_STRING"
            | "ARRAY_TO_STRING"
            | "STRING"
            | "LAX_STRING" => known(SqlType::Text),

            "MD5" | "SHA1" | "SHA256" | "SHA512" | "FROM_HEX" | "FROM_BASE64" | "FROM_BASE32" => {
                known(SqlType::Bytea)
            }

            "PARSE_JSON" | "TO_JSON" | "JSON_OBJECT" | "JSON_ARRAY" | "JSON_SET"
            | "JSON_REMOVE" | "JSON_STRIP_NULLS" => known(SqlType::Json),
            // JSON in, JSON out; STRING in, STRING out
            "JSON_EXTRACT" | "JSON_QUERY" => match arg(0) {
                ExpressionType::Known(SqlType::Json) => known(SqlType::Json),
                ty => string_like(ty),
            },
            "JSON_VALUE_ARRAY" | "JSON_EXTRACT_STRING_ARRAY" => {
                known(SqlType::Array(Box::new(SqlType::Text)))
            }

            "PARSE_DATE" | "DATE" | "DATE_FROM_UNIX_DATE" | "CURRENT_DATE" | "LAST_DAY" => {
                known(SqlType::Date)
            }
            "PARSE_TIMESTAMP" | "TIMESTAMP" | "TIMESTAMP_SECONDS" | "TIMESTAMP_MILLIS"
            | "TIMESTAMP_MICROS" | "TIMESTAMP_ADD" | "TIMESTAMP_SUB" | "TIMESTAMP_TRUNC"
            | "CURRENT_TIMESTAMP" => known(timestamp(true)),
            "PARSE_DATETIME" | "DATETIME" | "DATETIME_ADD" | "DATETIME_SUB" | "DATETIME_TRUNC"
            | "CURRENT_DATETIME" => known(timestamp(false)),
            "PARSE_TIME" | "TIME" | "TIME_ADD" | "TIME_SUB" | "TIME_TRUNC" | "CURRENT_TIME" => {
                known(time)
            }
            // DATE, or the DATETIME / TIMESTAMP they are given
            "DATE_ADD" | "DATE_SUB" | "DATE_TRUNC" => match arg(0) {
                ExpressionType::Known(t @ (SqlType::Date | SqlType::Timestamp { .. })) => known(t),
                ExpressionType::StringLiteral(_) => known(SqlType::Date),
                _ => Some(ExpressionType::Unknown),
            },

            "ARRAY_AGG" => array_of(arg(0)),
            "SPLIT" | "REGEXP_EXTRACT_ALL" => match string_like(arg(0)) {
                Some(ExpressionType::Known(t)) => known(SqlType::Array(Box::new(t))),
                _ => Some(ExpressionType::Unknown),
            },
            "GENERATE_DATE_ARRAY" => known(SqlType::Array(Box::new(SqlType::Date))),
            "GENERATE_TIMESTAMP_ARRAY" => known(SqlType::Array(Box::new(timestamp(true)))),
            "STRING_AGG" | "REGEXP_EXTRACT" | "REGEXP_REPLACE" | "REGEXP_SUBSTR" => {
                string_like(arg(0))
            }

            _ => None,
        }
    }

    /// Infer the type of the `n`th (unnamed) argument of a function
    fn infer_nth_arg_type(&self, func: &Function, n: usize) -> ExpressionType {
        if let FunctionArguments::List(list) = &func.args {
            if let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))) = list.args.get(n) {
                return self.infer_expr_type(expr);
            }
        }
        ExpressionType::Unknown
    }
}

/// Whether a type is a character string type
fn is_string(ty: &SqlType) -> bool {
    matches!(
        ty,
        SqlType::Text | SqlType::Varchar { .. } | SqlType::Char { .. }
    )
}

/// BigQuery date parts and week starts that functions take as bare words
/// (`DATE_TRUNC(d, ISOWEEK)`, `DATE_DIFF(a, b, WEEK(MONDAY))`)
pub(super) fn is_date_part(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "ISOWEEK"
            | "ISOYEAR"
            | "DAYOFWEEK"
            | "DAYOFYEAR"
            | "SUNDAY"
            | "MONDAY"
            | "TUESDAY"
            | "WEDNESDAY"
            | "THURSDAY"
            | "FRIDAY"
            | "SATURDAY"
    )
}

/// Split `>>` closing two nested type parameter lists (`STRUCT<a STRUCT<b INT64>>`)
/// into `> >`, outside string literals, quoted identifiers and comments.
///
/// sqlparser 0.53 rejects `>>` that closes a STRUCT nested as the last field of a
/// STRUCT when a comma follows (as in a column list), so a BigQuery table with such a
/// column would be skipped. Returns `None` when there is no `>>` to split.
pub(crate) fn split_closing_angle_brackets(sql: &str) -> Option<String> {
    if !sql.contains(">>") {
        return None;
    }
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                // Triple-quoted strings end at the next triple quote
                let triple = bytes.get(i..i + 3) == Some(&[quote, quote, quote][..]);
                i += if triple { 3 } else { 1 };
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if triple {
                        if bytes.get(i..i + 3) == Some(&[quote, quote, quote][..]) {
                            i += 3;
                            break;
                        }
                        i += 1;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = skip_line(bytes, i);
            }
            b'#' => i = skip_line(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| i + 2 + end + 2);
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                out.push_str(&sql[copied..i]);
                out.push_str("> ");
                copied = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[copied..]);
    Some(out)
}

/// Index just past the end of the line starting at `i`
fn skip_line(bytes: &[u8], i: usize) -> usize {
    bytes[i..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |end| i + end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nested_closing_brackets_outside_strings_and_comments() {
        assert_eq!(
            split_closing_angle_brackets("s STRUCT<a STRUCT<b INT64>>, x INT64 -- >>\n").as_deref(),
            Some("s STRUCT<a STRUCT<b INT64> >, x INT64 -- >>\n")
        );
        assert_eq!(
            split_closing_angle_brackets("ARRAY<STRUCT<a STRUCT<b INT64>>>").as_deref(),
            Some("ARRAY<STRUCT<a STRUCT<b INT64> > >")
        );
        assert_eq!(
            split_closing_angle_brackets("x STRING DEFAULT '>>' /* >> */").as_deref(),
            Some("x STRING DEFAULT '>>' /* >> */")
        );
        assert_eq!(split_closing_angle_brackets("a > b"), None);
    }
}
