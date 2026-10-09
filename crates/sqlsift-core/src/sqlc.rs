//! sqlc query names and parameters
//!
//! [sqlc](https://sqlc.dev) query files name each query with a comment before it:
//!
//! ```sql
//! -- name: GetPost :one
//! SELECT * FROM posts WHERE id = $1;
//! ```
//!
//! [`QueryNames`] finds these comments so that diagnostics can say which query they
//! are in.
//!
//! Queries may name their parameters with the sqlc macros `sqlc.arg(name)`,
//! `sqlc.narg(name)` and `sqlc.slice(name)` (the name may be quoted), and with
//! `@name` in PostgreSQL. [`mask_parameters`] rewrites them into untyped placeholders
//! (`$1`, or `?` for MySQL and SQLite) padded to the same length, so parser and
//! diagnostic locations still point at the original text.
//!
//! `@name` is a parameter with the PostgreSQL dialect only, and only when the `@` is
//! not part of an operator (`@>`, `<@`, `@@`, or `@` followed by a space): in MySQL
//! `@name` is a user variable, in SQLite a bind parameter, and sqlc supports neither
//! form there. Anything inside literals, quoted identifiers and comments is left
//! alone.

use std::borrow::Cow;

use crate::dialect::SqlDialect;

/// The sqlc query names of a file, by the line of their `-- name:` comment
pub(crate) struct QueryNames<'a> {
    /// (1-indexed line, name), in line order
    names: Vec<(usize, &'a str)>,
}

impl<'a> QueryNames<'a> {
    /// Find the `-- name: <Name> :<command>` comments in `sql`
    pub fn parse(sql: &'a str) -> Self {
        if !sql.contains("name:") {
            return Self { names: Vec::new() };
        }
        let names = sql
            .lines()
            .enumerate()
            .filter_map(|(i, line)| Some((i + 1, query_name(line)?)))
            .collect();
        Self { names }
    }

    /// Name of the query that `line` belongs to: the last `-- name:` comment at or
    /// before it
    pub fn at(&self, line: usize) -> Option<&'a str> {
        let index = self.names.partition_point(|&(l, _)| l <= line);
        index.checked_sub(1).map(|i| self.names[i].1)
    }
}

/// The query name of a `-- name: <Name> :<command>` line
fn query_name(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("--")?.trim_start();
    let rest = rest.strip_prefix("name:")?;
    let mut words = rest.split_whitespace();
    let name = words.next()?;
    let command = words.next()?.strip_prefix(':')?;
    let is_word = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    };
    (is_word(name) && is_word(command)).then_some(name)
}

/// The sqlc macros that stand for a parameter
const PARAMETER_MACROS: &[&str] = &["arg", "narg", "slice"];

/// Rewrite sqlc parameters into placeholders of the same length (see the module docs)
pub(crate) fn mask_parameters(sql: &str, dialect: SqlDialect) -> Cow<'_, str> {
    let postgres = dialect == SqlDialect::PostgreSQL;
    let mysql = dialect == SqlDialect::MySQL;
    if !(sql.contains('.') || postgres && sql.contains('@')) {
        return Cow::Borrowed(sql);
    }

    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut out: Option<Vec<u8>> = None;
    let placeholder: &[u8] = if postgres { b"$1" } else { b"?" };
    let prev = |i: usize| i.checked_sub(1).map(|p| bytes[p]);

    let mut i = 0;
    while i < len {
        let b = bytes[i];
        match b {
            b'-' if bytes.get(i + 1) == Some(&b'-') => i = skip_line(bytes, i),
            b'#' if mysql => i = skip_line(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = skip_block_comment(bytes, i, postgres),
            b'\'' | b'"' | b'`' => {
                // Backslash escapes: MySQL strings, PostgreSQL E'...' strings
                let backslash = b != b'`'
                    && (mysql
                        || postgres
                            && b == b'\''
                            && matches!(prev(i), Some(b'E' | b'e'))
                            && (i < 2 || !is_ident_byte(bytes[i - 2])));
                i = skip_quoted(bytes, i, backslash);
            }
            b'$' if postgres && !prev(i).is_some_and(is_ident_byte) => {
                i = skip_dollar_quoted(sql, i);
            }
            b'@' if postgres
                && !prev(i).is_some_and(|p| is_ident_byte(p) || matches!(p, b'@' | b'<'))
                && bytes
                    .get(i + 1)
                    .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_') =>
            {
                let end = i + 1 + word_len(&bytes[i + 1..]);
                mask(
                    out.get_or_insert_with(|| bytes.to_vec()),
                    i,
                    end,
                    placeholder,
                );
                i = end;
            }
            _ if is_ident_byte(b) => {
                let start = i;
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                if prev(start) != Some(b'.') && sql[start..i].eq_ignore_ascii_case("sqlc") {
                    if let Some(end) = parameter_macro_end(sql, i) {
                        mask(
                            out.get_or_insert_with(|| bytes.to_vec()),
                            start,
                            end,
                            placeholder,
                        );
                        i = end;
                    }
                }
            }
            _ => i += 1,
        }
    }

    match out {
        // Only ASCII spans were replaced, by ASCII, so this is still UTF-8
        Some(out) => Cow::Owned(String::from_utf8(out).unwrap_or_else(|_| sql.to_string())),
        None => Cow::Borrowed(sql),
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Length of the ASCII word (letters, digits, `_`) at the start of `bytes`
fn word_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
        .count()
}

/// End of a parameter macro call (`.arg(name)`, `.narg('name')`, `.slice(name)`)
/// that starts at `i`, right after `sqlc`
fn parameter_macro_end(sql: &str, i: usize) -> Option<usize> {
    let rest = sql[i..].strip_prefix('.')?;
    let name = &rest[..word_len(rest.as_bytes())];
    if !PARAMETER_MACROS
        .iter()
        .any(|m| name.eq_ignore_ascii_case(m))
    {
        return None;
    }
    let args = rest[name.len()..]
        .trim_start_matches([' ', '\t'])
        .strip_prefix('(')?;
    let close = args.find(')')?;
    let arg = args[..close].trim_matches([' ', '\t']);
    let unquoted = ['\'', '"']
        .iter()
        .find_map(|&q| arg.strip_prefix(q)?.strip_suffix(q))
        .unwrap_or(arg);
    let is_name = unquoted
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && word_len(unquoted.as_bytes()) == unquoted.len();
    is_name.then(|| sql.len() - args.len() + close + 1)
}

/// Replace `out[start..end]` with `placeholder`, padded with spaces
fn mask(out: &mut [u8], start: usize, end: usize, placeholder: &[u8]) {
    out[start..end].fill(b' ');
    out[start..start + placeholder.len()].copy_from_slice(placeholder);
}

/// Index of the line break that ends the comment starting at `i` (or the end)
fn skip_line(bytes: &[u8], i: usize) -> usize {
    bytes[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(bytes.len(), |p| i + p)
}

/// Index after the block comment starting at `i` (nested in PostgreSQL)
fn skip_block_comment(bytes: &[u8], mut i: usize, nested: bool) -> usize {
    i += 2;
    let mut depth = 1;
    while i < bytes.len() {
        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else if nested && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// Index after the quoted string or identifier starting at `i` (a doubled quote
/// stands for itself)
fn skip_quoted(bytes: &[u8], mut i: usize, backslash: bool) -> usize {
    let quote = bytes[i];
    i += 1;
    while i < bytes.len() {
        if backslash && bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == quote {
            if bytes.get(i + 1) != Some(&quote) {
                return i + 1;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    bytes.len()
}

/// Index after the dollar-quoted string (`$$...$$`, `$tag$...$tag$`) starting at `i`,
/// or after the `$` when it doesn't start one (`$1`)
fn skip_dollar_quoted(sql: &str, i: usize) -> usize {
    let bytes = sql.as_bytes();
    let closing = i + 1 + word_len(&bytes[i + 1..]);
    if bytes.get(closing) != Some(&b'$') || bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        return i + 1;
    }
    let tag = &sql[i..=closing];
    let body = closing + 1;
    sql[body..]
        .find(tag)
        .map_or(sql.len(), |p| body + p + tag.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(sql: &str, dialect: SqlDialect) -> String {
        let result = mask_parameters(sql, dialect).into_owned();
        assert_eq!(result.len(), sql.len());
        result
    }

    #[test]
    fn masks_parameter_macros() {
        let pg = SqlDialect::PostgreSQL;
        assert_eq!(rewrite("x = sqlc.arg(a)", pg), "x = $1         ");
        assert_eq!(
            rewrite("x = sqlc.narg('a')::int", pg),
            "x = $1            ::int"
        );
        assert_eq!(
            rewrite("x IN (sqlc.slice(ids))", SqlDialect::MySQL),
            "x IN (?              )"
        );
        assert_eq!(
            rewrite("x = sqlc.arg (a)", SqlDialect::SQLite),
            "x = ?           "
        );
    }

    #[test]
    fn masks_at_parameters_in_postgresql_only() {
        assert_eq!(
            rewrite("x > @a AND y=@b_2", SqlDialect::PostgreSQL),
            "x > $1 AND y=$1  "
        );
        assert_eq!(rewrite("x > @a", SqlDialect::MySQL), "x > @a");
        assert_eq!(rewrite("x > @a", SqlDialect::SQLite), "x > @a");
    }

    #[test]
    fn leaves_sql_alone() {
        for sql in [
            "SELECT a @> b, b <@ a, a<@b, t @@ q, t@@q, @ -1, a@b",
            "SELECT '@a sqlc.arg(a)', \"@a\", $$ @a $$, $t$ @a $t$, E'\\' @a', $1",
            "SELECT 1 -- @a sqlc.arg(a)\n/* @a /* sqlc.arg(a) */ @a */",
            "SELECT sqlc.embed(users), x.sqlc.arg(a), mysqlc.arg(a), sqlc.arg(a b), sqlc.arg(1)",
        ] {
            assert_eq!(rewrite(sql, SqlDialect::PostgreSQL), sql);
        }
        let mysql = "SELECT 'it\\'s sqlc.arg(a)', `sqlc.arg(a)` # sqlc.arg(a)";
        assert_eq!(rewrite(mysql, SqlDialect::MySQL), mysql);
    }

    #[test]
    fn parses_name_comments() {
        assert_eq!(query_name("-- name: GetPost :one"), Some("GetPost"));
        assert_eq!(
            query_name("  --name: list_posts :many  "),
            Some("list_posts")
        );
        assert_eq!(
            query_name("-- name: CreatePosts :copyfrom extra"),
            Some("CreatePosts")
        );
        assert_eq!(query_name("-- name: GetPost"), None);
        assert_eq!(query_name("-- name GetPost :one"), None);
        assert_eq!(query_name("SELECT 1 -- name: GetPost :one"), None);
    }

    #[test]
    fn finds_the_query_of_a_line() {
        let names = QueryNames::parse(
            "SELECT 1;\n-- name: A :one\nSELECT 2;\n\n-- name: B :many\nSELECT 3;",
        );
        assert_eq!(names.at(1), None);
        assert_eq!(names.at(2), Some("A"));
        assert_eq!(names.at(4), Some("A"));
        assert_eq!(names.at(6), Some("B"));
    }
}
