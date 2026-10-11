//! MySQL syntax support (MySQL dialect only)
//!
//! sqlparser doesn't understand some MySQL syntax that is common in dumps and
//! application queries. [`preprocess`] rewrites it into text the parser accepts:
//!
//! - Versioned comments (`/*!50001 CREATE VIEW ... */`, MariaDB's `/*M!100101 ... */`)
//!   are executed by MySQL, so their markers are blanked and their contents kept.
//! - The `@` of a quoted account name (`` `root`@`%` ``) becomes `.`, which sqlparser
//!   can tokenize.
//! - `ALGORITHM = ...`, `DEFINER = ...` and `SQL SECURITY ...` after `CREATE [OR
//!   REPLACE]` (views, triggers, routines) are blanked.
//! - SELECT modifiers (`STRAIGHT_JOIN`, `SQL_CALC_FOUND_ROWS`, `HIGH_PRIORITY`,
//!   `SQL_NO_CACHE`, ...) are blanked and `DISTINCTROW` becomes `DISTINCT`.
//! - The `STRAIGHT_JOIN` join operator becomes `JOIN`.
//! - Index hints (`USE | FORCE | IGNORE {INDEX | KEY} [FOR ...] (...)`) are blanked.
//! - The trailing `ORDER BY` and `LIMIT` of a single-table UPDATE are blanked.
//!
//! Every rewritten character becomes one character, so lines and character columns
//! are kept (byte lengths are kept too unless a blanked name has non-ASCII characters).
//!
//! `INSERT ... SET col = value, ...` is turned by [`insert_set_as_values`] into the
//! equivalent `INSERT ... (col, ...) VALUES (value, ...)`.

use std::borrow::Cow;

use sqlparser::ast::{AssignmentTarget, Parens, Query, SetExpr, Statement, Values};
use sqlparser::dialect::MySqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::psql::Preprocessed;

/// SELECT modifiers that don't change the result columns
const SELECT_MODIFIERS: &[&str] = &[
    "HIGH_PRIORITY",
    "STRAIGHT_JOIN",
    "SQL_SMALL_RESULT",
    "SQL_BIG_RESULT",
    "SQL_BUFFER_RESULT",
    "SQL_CACHE",
    "SQL_NO_CACHE",
    "SQL_CALC_FOUND_ROWS",
];

/// Rewrite MySQL-only syntax into SQL sqlparser accepts (see the module docs)
pub(crate) fn preprocess(sql: &str) -> Preprocessed<'_> {
    let unwrapped = rewrite_text(sql);
    let rewritten = rewrite_tokens(&unwrapped);
    match (unwrapped, rewritten) {
        (text, None) => Preprocessed::new(text, Vec::new()),
        (_, Some(text)) => Preprocessed::new(Cow::Owned(text), Vec::new()),
    }
}

/// Blank the markers of versioned comments (`/*!NNNNN` / `/*M!NNNNNN` and the
/// closing `*/`), keeping their contents, and turn the `@` of quoted account names
/// (`'user'@'host'`, which sqlparser can't tokenize) into a `.`
fn rewrite_text(sql: &str) -> Cow<'_, str> {
    if !["/*!", "/*M!", "'@", "`@", "\"@"]
        .iter()
        .any(|pattern| sql.contains(pattern))
    {
        return Cow::Borrowed(sql);
    }
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut out = bytes.to_vec();
    let mut in_versioned = false;
    let mut i = 0;
    while i < len {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                // Strings take backslash escapes; a doubled quote is an escaped quote
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        if bytes.get(i) != Some(&quote) {
                            break;
                        }
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
            }
            b'@' if i > 0
                && matches!(bytes[i - 1], b'\'' | b'"' | b'`')
                && matches!(bytes.get(i + 1), Some(b'\'' | b'"' | b'`')) =>
            {
                // An account name: the tokenizer can't read `user`@`host`
                out[i] = b'.';
                i += 1;
            }
            b'#' => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'*' if in_versioned && bytes.get(i + 1) == Some(&b'/') => {
                out[i] = b' ';
                out[i + 1] = b' ';
                in_versioned = false;
                i += 2;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let marker = match (bytes.get(i + 2), bytes.get(i + 3)) {
                    (Some(b'!'), _) => Some(3),
                    (Some(b'M'), Some(b'!')) => Some(4),
                    _ => None,
                };
                if let (Some(marker), false) = (marker, in_versioned) {
                    let mut end = i + marker;
                    while end < len && bytes[end].is_ascii_digit() {
                        end += 1;
                    }
                    out[i..end].fill(b' ');
                    in_versioned = true;
                    i = end;
                } else {
                    // An ordinary comment (MySQL comments don't nest)
                    i += 2;
                    while i < len && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                        i += 1;
                    }
                    i += 2;
                }
            }
            _ => i += 1,
        }
    }
    // Only ASCII bytes were replaced by ASCII bytes
    String::from_utf8(out).map_or(Cow::Borrowed(sql), Cow::Owned)
}

/// One rewrite: replace the characters in `start..end` (byte offsets) with
/// `replacement` padded with spaces to the same number of characters
struct Edit {
    start: usize,
    end: usize,
    replacement: &'static str,
}

/// Rewrite CREATE modifiers, SELECT modifiers, STRAIGHT_JOIN and index hints
/// (`None` if there is nothing to rewrite or `sql` can't be tokenized)
fn rewrite_tokens(sql: &str) -> Option<String> {
    let upper = sql.to_ascii_uppercase();
    if ![
        "ALGORITHM",
        "DEFINER",
        "SECURITY",
        "INDEX",
        "KEY",
        "DISTINCTROW",
        "LIMIT",
    ]
    .iter()
    .chain(SELECT_MODIFIERS)
    .any(|w| upper.contains(w))
    {
        return None;
    }

    let tokens = Tokenizer::new(&MySqlDialect {}, sql)
        .tokenize_with_location()
        .ok()?;
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(sql.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = |line: u64, column: u64| -> usize {
        let Some(&start) = usize::try_from(line)
            .ok()
            .and_then(|l| line_starts.get(l.saturating_sub(1)))
        else {
            return sql.len();
        };
        let column = usize::try_from(column).unwrap_or(usize::MAX);
        sql[start..]
            .char_indices()
            .nth(column.saturating_sub(1))
            .map_or(sql.len(), |(i, _)| start + i)
    };

    // Significant tokens with their byte ranges
    let tokens: Vec<(Token, usize, usize)> = tokens
        .into_iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .map(|t| {
            let start = offset(t.span.start.line, t.span.start.column);
            let end = offset(t.span.end.line, t.span.end.column);
            (t.token, start, end)
        })
        .collect();
    let word = |i: usize| match tokens.get(i) {
        Some((Token::Word(w), ..)) if w.quote_style.is_none() => w.value.to_ascii_uppercase(),
        _ => String::new(),
    };
    let is = |i: usize, token: &Token| tokens.get(i).is_some_and(|(t, ..)| t == token);

    let mut edits = Vec::new();
    let mut blank = |first: usize, last: usize, replacement: &'static str| {
        edits.push(Edit {
            start: tokens[first].1,
            end: tokens[last].2,
            replacement,
        });
    };

    // The trailing ORDER BY / LIMIT of the UPDATE being rewritten: (first, last)
    let mut update_tail: Option<(usize, usize)> = None;
    let mut k = 0;
    while k < tokens.len() {
        if let Some((first, last)) = update_tail.filter(|&(first, _)| first == k) {
            blank(first, last, "");
            update_tail = None;
            k = last + 1;
            continue;
        }
        match word(k).as_str() {
            "CREATE" => {
                let mut j = k + 1;
                if word(j) == "OR" && word(j + 1) == "REPLACE" {
                    j += 2;
                }
                loop {
                    match word(j).as_str() {
                        "ALGORITHM" if is(j + 1, &Token::Eq) && j + 2 < tokens.len() => {
                            blank(j, j + 2, "");
                            j += 3;
                        }
                        "DEFINER" if is(j + 1, &Token::Eq) && j + 2 < tokens.len() => {
                            // CURRENT_USER[()] or user@host, whose parts are adjacent
                            let mut last = j + 2;
                            if word(last) == "CURRENT_USER" {
                                if is(last + 1, &Token::LParen) && is(last + 2, &Token::RParen) {
                                    last += 2;
                                }
                            } else {
                                while tokens
                                    .get(last + 1)
                                    .is_some_and(|next| next.1 == tokens[last].2)
                                {
                                    last += 1;
                                }
                            }
                            blank(j, last, "");
                            j = last + 1;
                        }
                        "SQL"
                            if word(j + 1) == "SECURITY"
                                && matches!(word(j + 2).as_str(), "DEFINER" | "INVOKER") =>
                        {
                            blank(j, j + 2, "");
                            j += 3;
                        }
                        _ => break,
                    }
                }
                k = j;
            }
            "SELECT" => {
                let mut j = k + 1;
                loop {
                    let w = word(j);
                    if w == "DISTINCTROW" {
                        blank(j, j, "DISTINCT");
                    } else if SELECT_MODIFIERS.contains(&w.as_str()) {
                        blank(j, j, "");
                    } else if w != "ALL" && w != "DISTINCT" {
                        break;
                    }
                    j += 1;
                }
                k = j;
            }
            "STRAIGHT_JOIN" => {
                blank(k, k, "JOIN");
                k += 1;
            }
            // Single-table `UPDATE ... [ORDER BY ...] LIMIT n`: the trailing ORDER BY and
            // LIMIT only choose which rows are updated
            "UPDATE" if k == 0 || is(k - 1, &Token::SemiColon) => {
                let mut depth = 0usize;
                let mut seen_set = false;
                let mut tail = None;
                let mut j = k + 1;
                while j < tokens.len() && !(depth == 0 && is(j, &Token::SemiColon)) {
                    match &tokens[j].0 {
                        Token::LParen => depth += 1,
                        Token::RParen => depth = depth.saturating_sub(1),
                        _ if depth > 0 => {}
                        _ => match word(j).as_str() {
                            "SET" => seen_set = true,
                            "ORDER" if seen_set && word(j + 1) == "BY" => {
                                tail.get_or_insert(j);
                            }
                            "LIMIT" if seen_set => {
                                tail.get_or_insert(j);
                            }
                            _ => {}
                        },
                    }
                    j += 1;
                }
                update_tail = tail.map(|first| (first, j - 1));
                k += 1;
            }
            "USE" | "FORCE" | "IGNORE" if matches!(word(k + 1).as_str(), "INDEX" | "KEY") => {
                let mut j = k + 2;
                if word(j) == "FOR" {
                    match word(j + 1).as_str() {
                        "JOIN" => j += 2,
                        "ORDER" | "GROUP" if word(j + 2) == "BY" => j += 3,
                        _ => {}
                    }
                }
                if !is(j, &Token::LParen) {
                    k += 1;
                    continue;
                }
                let mut depth = 0usize;
                let mut end = None;
                for (i, (token, ..)) in tokens.iter().enumerate().skip(j) {
                    match token {
                        Token::LParen => depth += 1,
                        Token::RParen => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(i);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(end) = end else {
                    k += 1;
                    continue;
                };
                blank(k, end, "");
                k = end + 1;
            }
            _ => k += 1,
        }
    }

    if edits.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(sql.len());
    let mut copied = 0;
    for edit in edits {
        out.push_str(&sql[copied..edit.start]);
        // Keep line breaks (and so lines and columns) inside the rewritten range
        let mut replacement = edit.replacement.chars();
        for c in sql[edit.start..edit.end].chars() {
            if c == '\n' || c == '\r' {
                out.push(c);
            } else {
                out.push(replacement.next().unwrap_or(' '));
            }
        }
        copied = edit.end;
    }
    out.push_str(&sql[copied..]);
    Some(out)
}

/// Turn MySQL's `INSERT ... SET col = value, ...` (or `REPLACE ... SET`), which
/// sqlparser parses into [`sqlparser::ast::Insert::assignments`], into the equivalent
/// `INSERT ... (col, ...) VALUES (value, ...)`, so INSERT checks see one shape.
///
/// An assignment to a tuple of columns isn't valid there and leaves the statement
/// as parsed.
pub(crate) fn insert_set_as_values(statement: &mut Statement) {
    let Statement::Insert(insert) = statement else {
        return;
    };
    if insert.assignments.is_empty()
        || insert.source.is_some()
        || !insert.columns.is_empty()
        || insert
            .assignments
            .iter()
            .any(|a| !matches!(a.target, AssignmentTarget::ColumnName(_)))
    {
        return;
    }
    let mut row = Vec::with_capacity(insert.assignments.len());
    for assignment in std::mem::take(&mut insert.assignments) {
        if let AssignmentTarget::ColumnName(name) = assignment.target {
            insert.columns.push(name);
            row.push(assignment.value);
        }
    }
    insert.source = Some(Box::new(Query {
        with: None,
        body: Box::new(SetExpr::Values(Values {
            explicit_row: false,
            value_keyword: false,
            rows: vec![Parens::with_empty_span(row)],
        })),
        order_by: None,
        limit_clause: None,
        fetch: None,
        locks: Vec::new(),
        for_clause: None,
        settings: None,
        format_clause: None,
        pipe_operators: Vec::new(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(sql: &str) -> String {
        preprocess(sql).text.into_owned()
    }

    #[test]
    fn versioned_comments_are_unwrapped() {
        assert_eq!(
            rewrite("/*!50001 VIEW `v` AS select 1 */;"),
            "         VIEW `v` AS select 1   ;"
        );
        assert_eq!(
            rewrite("/*M!100101 SET x = 1 */"),
            "           SET x = 1   "
        );
        assert_eq!(rewrite("TO 'a'@'%', `b`@`h`"), "TO 'a'.'%', `b`.`h`");
        assert_eq!(rewrite("SELECT @a, @`b`"), "SELECT @a, @`b`");
        // Ordinary comments, optimizer hints and strings are left alone
        for sql in [
            "SELECT /*+ BKA(t) */ 1",
            "SELECT '/*!50001 x */'",
            "-- /*!50001 x\nSELECT 1",
        ] {
            assert_eq!(rewrite(sql), sql);
        }
    }

    #[test]
    fn create_view_modifiers_are_blanked() {
        let sql =
            "CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`%` SQL SECURITY DEFINER VIEW v AS SELECT 1";
        let out = rewrite(sql);
        assert_eq!(out.len(), sql.len());
        assert_eq!(
            out.split_whitespace().collect::<Vec<_>>(),
            ["CREATE", "VIEW", "v", "AS", "SELECT", "1"]
        );
    }

    #[test]
    fn index_hints_and_modifiers_keep_columns() {
        let sql = "SELECT STRAIGHT_JOIN a FROM t FORCE INDEX (i)\nSTRAIGHT_JOIN u USE KEY FOR JOIN (j, k) WHERE x";
        let out = rewrite(sql);
        assert_eq!(out.len(), sql.len());
        assert_eq!(out.find('\n'), sql.find('\n'));
        assert_eq!(
            out.split_whitespace().collect::<Vec<_>>(),
            ["SELECT", "a", "FROM", "t", "JOIN", "u", "WHERE", "x"]
        );
    }

    #[test]
    fn insert_set_becomes_values() {
        let parse = |sql: &str| {
            let mut statements =
                sqlparser::parser::Parser::parse_sql(&MySqlDialect {}, sql).expect("parses");
            insert_set_as_values(&mut statements[0]);
            statements.remove(0).to_string()
        };
        assert_eq!(
            parse("INSERT INTO t SET a = 1, b = 'x';"),
            "INSERT INTO t (a, b) VALUES (1, 'x')"
        );
        assert_eq!(
            parse("INSERT INTO t VALUES (1)"),
            "INSERT INTO t VALUES (1)"
        );
    }
}
