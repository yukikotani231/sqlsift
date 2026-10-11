//! BigQuery semantics of the resolver (alpha)
//!
//! - `a.b.c` paths: `a` is a table alias or a column, and what follows the column
//!   are STRUCT field names, which sqlsift doesn't model (so never reports)
//! - tables in datasets the schema doesn't define, and wildcard tables
//!   (`` `dataset.events_*` ``), are unknown rather than missing
//! - return types of common BigQuery functions, and the date parts (`DAY`,
//!   `ISOWEEK`, `WEEK(MONDAY)`, ...) their arguments take as bare words
//!
//! `` `project.dataset.table` `` names are split and resolved like any qualified
//! name (`split_dotted_idents`, `Scope::qualified_relation`); BigQuery catalogs are
//! not strict about schemas, so a dataset prefix the schema input leaves out still
//! finds the table.

use std::collections::HashMap;

use sqlparser::ast::{Function, FunctionArg, FunctionArgExpr, FunctionArguments, Ident};

use crate::dialect::SqlDialect;
use crate::error::Span;
use crate::schema::{split_dotted_idents, QualifiedName};
use crate::types::SqlType;

use super::resolver::Resolver;
use super::scope::{split_column_ref, ColumnLookup, ExpressionType};

impl Resolver<'_> {
    /// Resolve a BigQuery path `a.b[.c...]`: `alias.column[.field...]`,
    /// `column.field[.field...]`, or `[[project.]dataset.]table.column`
    pub(super) fn bigquery_path(&mut self, idents: &[Ident]) {
        let parts = split_dotted_idents(idents);
        let [first, second, ..] = parts.as_ref() else {
            return;
        };
        if self.scope.relation(&first.value).is_some() {
            self.column(Some((None, first)), second);
        } else if self.is_value_name(first) {
            // A STRUCT column (or output alias): the rest are its fields
            self.column(None, first);
        } else if let Some((schema, table, column)) = split_column_ref(&parts) {
            // A table named with its dataset, or else an unknown alias
            self.column(Some((schema, table)), column);
        }
    }

    /// Resolve the path of `path.*` that is not a relation in scope: a STRUCT
    /// column (`info.*`, `t.info.*`, `info.inner.*`), whose fields sqlsift doesn't
    /// model. Returns false if the path doesn't start with a relation or a column.
    pub(super) fn bigquery_struct_star(&mut self, parts: &[Ident]) -> bool {
        match parts {
            [first, second, ..] if self.scope.relation(&first.value).is_some() => {
                self.column(Some((None, first)), second);
                true
            }
            [first, ..] if self.is_value_name(first) => {
                self.column(None, first);
                true
            }
            _ => false,
        }
    }

    /// Type of a BigQuery path `a.b[.c...]`: a column's type for `alias.column` and
    /// `dataset.table.column`, unknown for STRUCT fields
    pub(super) fn infer_bigquery_path(&self, idents: &[Ident]) -> ExpressionType {
        let parts = split_dotted_idents(idents);
        match parts.as_ref() {
            [table, column] if self.scope.relation(&table.value).is_some() => {
                self.infer_qualified_column(None, table, column)
            }
            [first, _, _, ..]
                if self.scope.relation(&first.value).is_none() && !self.is_value_name(first) =>
            {
                match split_column_ref(&parts) {
                    Some((schema, table, column)) => {
                        self.infer_qualified_column(schema, table, column)
                    }
                    None => ExpressionType::Unknown,
                }
            }
            _ => ExpressionType::Unknown,
        }
    }

    /// Whether an unqualified name is a column (or an output column alias) in scope.
    /// In Databricks (where struct paths are rarer than BigQuery's) a name that only
    /// may be a column of a relation with unknown columns is not one, so a misspelled
    /// alias is still reported.
    fn is_value_name(&self, ident: &Ident) -> bool {
        let is_column = match self.scope.column(&ident.value, self.dialect) {
            ColumnLookup::NotFound => false,
            ColumnLookup::Unknown => self.dialect != SqlDialect::Databricks,
            _ => true,
        };
        is_column
            || self.scope.current_ref().is_some_and(|f| {
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
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
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

/// Index just past the string literal or quoted identifier starting at `i`
/// (triple-quoted strings end at the next triple quote)
fn skip_quoted(bytes: &[u8], mut i: usize) -> usize {
    let quote = bytes[i];
    let triple = bytes.get(i..i + 3) == Some(&[quote, quote, quote][..]);
    i += if triple { 3 } else { 1 };
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if triple {
            if bytes.get(i..i + 3) == Some(&[quote, quote, quote][..]) {
                return i + 3;
            }
            i += 1;
        } else if bytes[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// Field name the analyzer gives `expr.*` (the fields of a STRUCT expression),
/// which [`rewrite_query`] turns into `(expr).__sqlsift_struct_star__`
pub(super) const STRUCT_STAR_FIELD: &str = "__sqlsift_struct_star__";

/// BigQuery query text rewritten into syntax sqlparser 0.53 parses, with the
/// columns where characters were inserted
pub(crate) struct Rewritten {
    pub(crate) text: String,
    /// (line, character column of the first inserted character, number inserted),
    /// in the rewritten text, 1-indexed
    insertions: Vec<(usize, usize, usize)>,
}

impl Rewritten {
    /// Move a location in the rewritten text to the original text
    pub(crate) fn map_span(&self, span: &mut Span) {
        if span.line == 0 {
            return;
        }
        let back: usize = self
            .insertions
            .iter()
            .filter(|(line, ..)| *line == span.line)
            .map(|&(_, column, count)| span.column.saturating_sub(column).min(count))
            .sum();
        span.column -= back;
    }
}

/// A change to the query text: `remove` bytes at `at` replaced by `insert`
struct Edit {
    at: usize,
    remove: usize,
    insert: String,
}

/// Rewrite BigQuery syntax sqlparser 0.53 doesn't parse, outside strings, quoted
/// identifiers and comments (`None` if there is none):
///
/// - `ANY TYPE` parameters of temporary functions: `ANY_TYPE`, a type name
/// - `<<` / `>>` shifts: `|` (an integer or BYTES operator like them)
/// - typed array literals (`ARRAY<STRUCT<a INT64>>[...]`): the type is blanked,
///   leaving an array literal; `>>` closing nested type parameters elsewhere
///   (`CAST(x AS ARRAY<STRUCT<a INT64>>)`) becomes `> >`
/// - field access on a call or subscript result: `f(x).a.b` becomes
///   `((f(x)).a).b`, and `f(x).*` becomes `(f(x)).__sqlsift_struct_star__`
///   ([`STRUCT_STAR_FIELD`]) with its `EXCEPT (...)` / `REPLACE (...)` blanked
///
/// Lines never change; inserted characters are recorded so diagnostics can be
/// moved back with [`Rewritten::map_span`].
pub(crate) fn rewrite_query(sql: &str) -> Option<Rewritten> {
    let bytes = sql.as_bytes();
    let mut edits: Vec<Edit> = Vec::new();
    // Unclosed `(` / `[`, and the start of the expression each closed group ends
    // (`f(x)[OFFSET(0)]` -> start of `f`), keyed by the closing bracket's index
    let mut opens: Vec<usize> = Vec::new();
    let mut expr_starts: HashMap<usize, usize> = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'-' if bytes.get(i + 1) == Some(&b'-') => i = skip_line(bytes, i),
            b'#' => i = skip_line(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = sql[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| i + 2 + end + 2);
            }
            b'(' | b'[' => {
                opens.push(i);
                i += 1;
            }
            close @ (b')' | b']') => {
                let group = opens.pop().map(|open| {
                    let start = expression_start(bytes, open, &expr_starts);
                    expr_starts.insert(i, start);
                    (open, start)
                });
                i += 1;
                if let Some((open, start)) = group {
                    if bytes.get(i) == Some(&b'.') {
                        let group = Group {
                            open,
                            start,
                            close: i - 1,
                            subscript: close == b']',
                        };
                        i = field_access(sql, &group, &mut edits);
                    }
                }
            }
            b'<' | b'>' if bytes.get(i + 1) == Some(&bytes[i]) => {
                edits.push(Edit {
                    at: i,
                    remove: 2,
                    insert: "| ".into(),
                });
                i += 2;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let end = word_end(bytes, i);
                let word = &sql[i..end];
                let next = skip_whitespace(bytes, end);
                let array = word.eq_ignore_ascii_case("ARRAY");
                if (array || word.eq_ignore_ascii_case("STRUCT")) && bytes.get(next) == Some(&b'<')
                {
                    i = type_parameters(sql, i, next, array, &mut edits);
                } else if word.eq_ignore_ascii_case("ANY")
                    && matches!(bytes.get(end), Some(b' ' | b'\t'))
                    && sql
                        .get(end + 1..end + 5)
                        .is_some_and(|w| w.eq_ignore_ascii_case("TYPE"))
                    && !bytes
                        .get(end + 5)
                        .is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    edits.push(Edit {
                        at: end,
                        remove: 1,
                        insert: "_".into(),
                    });
                    i = end + 5;
                } else {
                    i = end;
                }
            }
            _ => i += 1,
        }
    }
    if edits.is_empty() {
        return None;
    }

    // Stable: insertions at the same place stay in the order they were made
    edits.sort_by_key(|e| e.at);
    let mut text = String::with_capacity(sql.len() + 16);
    let mut insertions = Vec::new();
    let mut copied = 0;
    // Line of the end of `text` and where that line starts in it
    let (mut line, mut line_start) = (1, 0);
    for edit in edits {
        if edit.at < copied {
            continue;
        }
        let unchanged = &sql[copied..edit.at];
        line += unchanged.matches('\n').count();
        if let Some(n) = unchanged.rfind('\n') {
            line_start = text.len() + n + 1;
        }
        text.push_str(unchanged);
        copied = edit.at + edit.remove;
        let inserted = edit.insert.chars().count();
        let added = inserted.saturating_sub(edit.remove);
        if added > 0 {
            // Location of the added characters in the rewritten text (edits that
            // add characters have no line breaks)
            let column = text[line_start..].chars().count() + 1 + inserted - added;
            insertions.push((line, column, added));
        }
        line += edit.insert.matches('\n').count();
        if let Some(n) = edit.insert.rfind('\n') {
            line_start = text.len() + n + 1;
        }
        text.push_str(&edit.insert);
    }
    text.push_str(&sql[copied..]);
    Some(Rewritten { text, insertions })
}

/// A bracketed group followed by a `.`
struct Group {
    /// Index of `(` / `[`
    open: usize,
    /// Start of the expression the group ends (see [`expression_start`])
    start: usize,
    /// Index of `)` / `]`
    close: usize,
    /// Whether it is a `[...]` subscript
    subscript: bool,
}

/// Start of the expression a bracketed group opened at `open` ends: the function
/// name before `(` (`SAFE.f(`, `` `p.ds.f`( ``), the expression a `[` subscripts,
/// or `open` itself
fn expression_start(bytes: &[u8], open: usize, expr_starts: &HashMap<usize, usize>) -> usize {
    let mut start = open;
    while let Some(&prev) = start.checked_sub(1).and_then(|p| bytes.get(p)) {
        match prev {
            b')' | b']' if start == open => match expr_starts.get(&(start - 1)) {
                Some(&s) => return s,
                None => break,
            },
            b'`' => match bytes[..start - 1].iter().rposition(|&c| c == b'`') {
                Some(quote) => start = quote,
                None => break,
            },
            c if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' => start -= 1,
            _ => break,
        }
    }
    if bytes.get(start) == Some(&b'.') {
        return open;
    }
    start
}

/// Rewrite the field accesses after `group` (a `.` follows it); returns where to
/// continue scanning
fn field_access(sql: &str, group: &Group, edits: &mut Vec<Edit>) -> usize {
    let bytes = sql.as_bytes();
    let mut key_ends = Vec::new();
    let mut star = None;
    let mut j = group.close + 1;
    while bytes.get(j) == Some(&b'.') {
        match bytes.get(j + 1) {
            Some(b'*') => {
                star = Some(j + 1);
                j += 2;
                break;
            }
            Some(b'`') => j = skip_quoted(bytes, j + 1),
            Some(&c) if c.is_ascii_alphabetic() || c == b'_' => j = word_end(bytes, j + 1),
            _ => break,
        }
        key_ends.push(j);
    }
    // Field access on subscripts and one field of `(expr)` parse already
    let parses = key_ends.is_empty()
        || group.subscript
        || (group.start == group.open && key_ends.len() == 1);
    if star.is_none() && parses {
        return group.close + 1;
    }
    let accesses = key_ends.len() + usize::from(star.is_some());
    edits.push(Edit {
        at: group.start,
        remove: 0,
        insert: "(".repeat(accesses),
    });
    // `)` after the expression and after each access but the last
    for at in std::iter::once(group.close + 1)
        .chain(key_ends.iter().copied())
        .take(accesses)
    {
        edits.push(Edit {
            at,
            remove: 0,
            insert: ")".into(),
        });
    }
    if let Some(star) = star {
        edits.push(Edit {
            at: star,
            remove: 1,
            insert: STRUCT_STAR_FIELD.into(),
        });
        // `.* EXCEPT (a) REPLACE (b AS c)` name STRUCT fields: blanked
        loop {
            let word_start = skip_whitespace(bytes, j);
            let end = word_end(bytes, word_start);
            let word = &sql[word_start..end];
            let paren = skip_whitespace(bytes, end);
            if !(word.eq_ignore_ascii_case("EXCEPT") || word.eq_ignore_ascii_case("REPLACE"))
                || bytes.get(paren) != Some(&b'(')
            {
                break;
            }
            let Some(close) = matching_paren(bytes, paren) else {
                break;
            };
            edits.push(blank(sql, word_start, close + 1));
            j = close + 1;
        }
    }
    j
}

/// Skip the type parameters of `ARRAY<...>` / `STRUCT<...>` (the keyword at
/// `word`, `<` at `lt`), rewriting them where sqlparser needs it; returns where to
/// continue scanning
fn type_parameters(sql: &str, word: usize, lt: usize, array: bool, edits: &mut Vec<Edit>) -> usize {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut double_closes = Vec::new();
    let mut j = lt;
    while j < bytes.len() {
        match bytes[j] {
            b'<' => depth += 1,
            b'>' => {
                // `>>` tokenizes as a shift operator
                if bytes.get(j + 1) == Some(&b'>') && depth >= 2 {
                    double_closes.push(j + 1);
                }
                depth -= 1;
            }
            b'\'' | b'"' | b'`' => {
                j = skip_quoted(bytes, j);
                continue;
            }
            b';' => return lt + 1,
            _ => {}
        }
        j += 1;
        if depth == 0 {
            break;
        }
    }
    if depth != 0 {
        return lt + 1;
    }
    if array && bytes.get(skip_whitespace(bytes, j)) == Some(&b'[') {
        // A typed array literal: an array literal
        edits.push(blank(sql, word, j));
    } else {
        for at in double_closes {
            edits.push(Edit {
                at,
                remove: 0,
                insert: " ".into(),
            });
        }
    }
    j
}

/// An edit replacing `start..end` with spaces, keeping line breaks
fn blank(sql: &str, start: usize, end: usize) -> Edit {
    Edit {
        at: start,
        remove: end - start,
        insert: sql[start..end]
            .chars()
            .map(|c| if c == '\n' || c == '\r' { c } else { ' ' })
            .collect(),
    }
}

/// Index of the `)` matching the `(` at `open`, skipping strings and comments
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = open;
    while j < bytes.len() {
        match bytes[j] {
            b'\'' | b'"' | b'`' => {
                j = skip_quoted(bytes, j);
                continue;
            }
            b'-' if bytes.get(j + 1) == Some(&b'-') => {
                j = skip_line(bytes, j);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Index just past the identifier characters starting at `i`
fn word_end(bytes: &[u8], i: usize) -> usize {
    bytes[i..]
        .iter()
        .position(|&c| !(c.is_ascii_alphanumeric() || c == b'_'))
        .map_or(bytes.len(), |n| i + n)
}

/// Index of the first non-whitespace byte at or after `i`
fn skip_whitespace(bytes: &[u8], i: usize) -> usize {
    let i = i.min(bytes.len());
    bytes[i..]
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .map_or(bytes.len(), |n| i + n)
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

    fn rewritten(sql: &str) -> Option<String> {
        rewrite_query(sql).map(|r| r.text)
    }

    #[test]
    fn rewrites_syntax_sqlparser_does_not_parse() {
        assert_eq!(rewritten("SELECT a FROM t WHERE (x).y > 0"), None);
        assert_eq!(rewritten("SELECT a[OFFSET(0)].b FROM t -- f(x).y"), None);
        assert_eq!(
            rewritten("f(x ANY TYPE, y any\ttype)").as_deref(),
            Some("f(x ANY_TYPE, y any_type)")
        );
        assert_eq!(rewritten("1 << 2 >> 3").as_deref(), Some("1 |  2 |  3"));
        assert_eq!(
            rewritten("SAFE.f(x).a.b").as_deref(),
            Some("((SAFE.f(x)).a).b")
        );
        assert_eq!(
            rewritten("`p.ds.f`(x)[OFFSET(0)].* EXCEPT (a)\nREPLACE (b AS c), d").as_deref(),
            Some(
                "(`p.ds.f`(x)[OFFSET(0)]).__sqlsift_struct_star__           \n                , d"
            )
        );
        assert_eq!(
            rewritten("ARRAY<STRUCT<a INT64>>[(1)], ARRAY<STRING>[]").as_deref(),
            Some("                      [(1)],              []")
        );
        assert_eq!(
            rewritten("CAST(x AS ARRAY<STRUCT<a STRUCT<b INT64>>>)").as_deref(),
            Some("CAST(x AS ARRAY<STRUCT<a STRUCT<b INT64> > >)")
        );
    }

    #[test]
    fn maps_locations_back_over_inserted_characters() {
        let rewritten = rewrite_query("SELECT f(x).a, b\nFROM t WHERE g(y).c = d").unwrap();
        assert_eq!(
            rewritten.text,
            "SELECT (f(x)).a, b\nFROM t WHERE (g(y)).c = d"
        );
        let mapped = |line, column| {
            let mut span = Span::with_location(line, column, 1);
            rewritten.map_span(&mut span);
            (span.line, span.column)
        };
        assert_eq!(mapped(1, 8), (1, 8));
        assert_eq!(mapped(1, 9), (1, 8));
        assert_eq!(mapped(1, 18), (1, 16));
        assert_eq!(mapped(2, 26), (2, 24));
    }
}
