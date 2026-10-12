//! SQL embedded in Python: string literals passed to database calls
//!
//! Python code keeps SQL in string literals, as in
//!
//! ```python
//! cur.execute("SELECT id, name FROM users WHERE id = %s", (user_id,))
//! row = await conn.fetchrow(
//!     """
//!     SELECT id, name
//!     FROM users
//!     WHERE id = $1
//!     """,
//!     user_id,
//! )
//! stmt = text("SELECT id FROM users WHERE name = :name")
//! ```
//!
//! which are checked by [`Analyzer::analyze_python`](crate::Analyzer::analyze_python).
//! Guessing which strings are SQL would report on strings that aren't, so only
//! the first argument of a call of one of the configured function names is
//! extracted (the name is the last part of the callee: `execute` matches
//! `cur.execute(...)` and `self.db.execute(...)`; see [`DEFAULT_FUNCTIONS`]), and
//! only when it
//!
//! - is a plain string literal (`'...'`, `"..."`, `'''...'''`, `"""..."""`, with
//!   an `r` or `u` prefix or none), or several of them side by side (implicit
//!   concatenation: `"SELECT a " "FROM t"`). f-strings, byte strings and anything
//!   built with `+`, `%` or `.format()` are skipped: their SQL is only known at
//!   run time.
//! - starts with a statement keyword (`SELECT`, `WITH`, `INSERT`, ...).
//!
//! Driver placeholders become untyped parameters: `%s` and `%(name)s` (psycopg,
//! MySQLdb, PyMySQL, Django), `:name` (SQLAlchemy `text()`, sqlite3) and `?`
//! (sqlite3) and `$1` (asyncpg). After `IN`, a placeholder is a list
//! (`IN %s` with a tuple in psycopg) and becomes `IN ($1)`.
//!
//! The file is turned into SQL text with the same lines and character columns,
//! as for TypeScript (see [`crate::embedded`]): everything outside the extracted
//! literals becomes spaces (line breaks are kept), the first opening and last
//! closing quotes of a literal become `;`, and `#` comments holding a
//! `sqlsift:disable` directive become `--` comments. Escapes become the
//! character they stand for (`\n` and `\t` a space): the rest of their word
//! moves left, and the following words keep their columns.

use std::path::Path;

use crate::dialect::SqlDialect;
use crate::embedded::{directive, finish, starts_with_statement, Extracted};

/// File extensions whose SQL is extracted
pub const EXTENSIONS: &[&str] = &["py"];

/// Function names whose first argument is extracted by default: the query
/// methods of DB-API cursors (psycopg, sqlite3, MySQLdb, PyMySQL) and of asyncpg
/// connections, SQLAlchemy's `text()` and `exec_driver_sql()`, and Django's
/// `raw()`
pub const DEFAULT_FUNCTIONS: &[&str] = &[
    "execute",
    "executemany",
    "executescript",
    "exec_driver_sql",
    "fetch",
    "fetchrow",
    "fetchval",
    "raw",
    "text",
];

/// Whether `path` is a Python file (see [`EXTENSIONS`])
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::python::is_python_file;
///
/// assert!(is_python_file(Path::new("app/db.py")));
/// assert!(!is_python_file(Path::new("app/db.rs")));
/// ```
pub fn is_python_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether `source` passes SQL to one of `functions` (see the module docs)
///
/// ```
/// use sqlsift_core::python::{has_sql, DEFAULT_FUNCTIONS};
///
/// assert!(has_sql("cur.execute('SELECT 1')", DEFAULT_FUNCTIONS));
/// assert!(!has_sql("cur.execute(query)", DEFAULT_FUNCTIONS));
/// ```
pub fn has_sql<S: AsRef<str>>(source: &str, functions: &[S]) -> bool {
    extract(source, functions, SqlDialect::PostgreSQL)
        .text
        .contains(';')
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A string literal in the source
struct Literal {
    /// Index of its first character (prefix or quote)
    start: usize,
    /// Index of its opening quote
    open: usize,
    /// Number of quote characters (1 or 3)
    quotes: usize,
    /// Index after its last character
    end: usize,
    /// `r` prefix: backslashes are kept
    raw: bool,
    /// Whether it can be SQL: no `f`, `b` or `t` prefix, and closed
    plain: bool,
}

impl Literal {
    fn content(&self) -> std::ops::Range<usize> {
        self.open + self.quotes..self.end.saturating_sub(self.quotes)
    }
}

/// Extract the SQL of the string literals passed to one of `functions` (see the
/// module docs)
pub(crate) fn extract<S: AsRef<str>>(
    source: &str,
    functions: &[S],
    dialect: SqlDialect,
) -> Extracted {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut out: Vec<char> = chars
        .iter()
        .map(|&c| if matches!(c, '\n' | '\r') { c } else { ' ' })
        .collect();
    let mut directives = Vec::new();
    let mut identifiers = Vec::new();

    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '#' {
            let start = i;
            while i < n && !matches!(chars[i], '\n' | '\r') {
                i += 1;
            }
            // `# sqlsift:disable ...` becomes `-- sqlsift:disable ...` (the space is
            // needed for the second `-`)
            if chars.get(start + 1).is_some_and(|c| c.is_whitespace()) && start + 2 <= i {
                directives.extend(directive(&chars[start + 2..i], start, Some(i)));
            }
        } else if let Some(first) = literal_at(&chars, i) {
            // A literal, or several side by side
            let mut run = vec![first];
            let mut j = skip_space(&chars, run[0].end);
            while let Some(next) = literal_at(&chars, j) {
                j = skip_space(&chars, next.end);
                run.push(next);
            }
            i = run.last().map_or(i + 1, |l| l.end);
            let sql = run.iter().all(|l| l.plain)
                && is_sql_argument(&chars, run[0].start, functions)
                && ends_argument(&chars, i)
                && starts_with_statement(&chars, run[0].content().start);
            if sql {
                let ranges = copy_run(&chars, &mut out, &run);
                for range in ranges {
                    rewrite_placeholders(&mut out, range, dialect, &mut identifiers);
                }
            }
        } else if is_ident_char(c) {
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    finish(&chars, out, &directives, identifiers, Vec::new())
}

/// The string literal starting at `i` (its prefix or quote), if one does
fn literal_at(chars: &[char], i: usize) -> Option<Literal> {
    if i > 0 && is_ident_char(chars[i - 1]) {
        return None;
    }
    let mut open = i;
    while open < chars.len() && open - i < 2 && chars[open].is_ascii_alphabetic() {
        open += 1;
    }
    let quote = *chars.get(open)?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let prefix: String = chars[i..open].iter().collect::<String>().to_lowercase();
    if !matches!(
        prefix.as_str(),
        "" | "r" | "u" | "f" | "b" | "t" | "br" | "rb" | "fr" | "rf" | "tr" | "rt"
    ) {
        return None;
    }
    let raw = prefix.contains('r');
    let plain = !prefix.contains(['f', 'b', 't']);
    let quotes = if chars.get(open + 1) == Some(&quote) && chars.get(open + 2) == Some(&quote) {
        3
    } else {
        1
    };
    let n = chars.len();
    let mut j = open + quotes;
    while j < n {
        match chars[j] {
            // (a raw string's backslash still keeps the quote after it in the
            // string)
            '\\' => j += 2,
            c if c == quote
                && (quotes == 1
                    || (chars.get(j + 1) == Some(&quote) && chars.get(j + 2) == Some(&quote))) =>
            {
                return Some(Literal {
                    start: i,
                    open,
                    quotes,
                    end: j + quotes,
                    raw,
                    plain,
                });
            }
            '\n' if quotes == 1 => break,
            _ => j += 1,
        }
    }
    // Unterminated: never SQL
    Some(Literal {
        start: i,
        open,
        quotes,
        end: j.min(n),
        raw,
        plain: false,
    })
}

/// The index of the next character after whitespace, comments and line
/// continuations from `i` (implicit concatenation spans them inside
/// parentheses)
fn skip_space(chars: &[char], mut i: usize) -> usize {
    loop {
        match chars.get(i) {
            Some(c) if c.is_whitespace() || *c == '\\' => i += 1,
            Some('#') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            _ => return i,
        }
    }
}

/// Whether the literal run ending at `end` is a whole argument: followed by `,`
/// or `)`, not by an operator (`+`, `%`) or a method call (`.format()`)
fn ends_argument(chars: &[char], end: usize) -> bool {
    matches!(chars.get(skip_space(chars, end)), Some(',' | ')'))
}

/// Whether the literal starting at `i` is the first argument of a call of one
/// of `functions`: `name(` or `obj.name(` before it
fn is_sql_argument<S: AsRef<str>>(chars: &[char], i: usize, functions: &[S]) -> bool {
    let end = skip_space_back(chars, i);
    if end == 0 || chars[end - 1] != '(' {
        return false;
    }
    let name_end = skip_space_back(chars, end - 1);
    let mut start = name_end;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    if start == name_end {
        return false;
    }
    let name: String = chars[start..name_end].iter().collect();
    if !functions.iter().any(|f| f.as_ref() == name) {
        return false;
    }
    // Not a definition (`def execute(...)`)
    let before = skip_space_back(chars, start);
    let mut word_start = before;
    while word_start > 0 && is_ident_char(chars[word_start - 1]) {
        word_start -= 1;
    }
    let word: String = chars[word_start..before].iter().collect();
    word != "def" && word != "class"
}

/// The index after the last non-whitespace character of `chars[..end]` (line
/// continuations included)
fn skip_space_back(chars: &[char], mut end: usize) -> usize {
    while end > 0 && (chars[end - 1].is_whitespace() || chars[end - 1] == '\\') {
        end -= 1;
    }
    end
}

/// Copy the text of the literals of `run` to `out`: the first opening and the
/// last closing quote become `;`, escapes the character they stand for. Returns
/// the ranges of `out` holding SQL text.
fn copy_run(chars: &[char], out: &mut [char], run: &[Literal]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    for literal in run {
        let content = literal.content();
        if literal.raw {
            out[content.clone()].copy_from_slice(&chars[content.clone()]);
        } else {
            copy_escaped(chars, out, content.clone());
        }
        ranges.push(content);
    }
    if let (Some(first), Some(last)) = (run.first(), run.last()) {
        out[first.open] = ';';
        out[last.end - 1] = ';';
    }
    ranges
}

/// Copy `range` of `chars` to `out`, escapes becoming the character they stand
/// for, written right after the text before them so the word they are in stays
/// whole; the spaces this saves go at the next whitespace, so that the next word
/// is in its place again
pub(crate) fn copy_escaped(chars: &[char], out: &mut [char], range: std::ops::Range<usize>) {
    let end = range.end;
    let mut at = range.start;
    let mut j = range.start;
    while j < end {
        match chars[j] {
            '\\' => {
                let escape_end = escape_end(chars, j).min(end);
                match unescape(&chars[j..escape_end]) {
                    Some(c) => {
                        out[at] = c;
                        at += 1;
                    }
                    None => at = escape_end,
                }
                j = escape_end;
            }
            c if c.is_whitespace() => {
                out[j] = c;
                j += 1;
                at = j;
            }
            c => {
                out[at] = c;
                at += 1;
                j += 1;
            }
        }
    }
}

/// The index after the escape starting with the backslash at `i`
fn escape_end(chars: &[char], i: usize) -> usize {
    let n = chars.len();
    match chars.get(i + 1) {
        Some('x') => (i + 4).min(n),
        Some('u') => (i + 6).min(n),
        Some('U') => (i + 10).min(n),
        Some('N') => chars[i..]
            .iter()
            .position(|&c| c == '}')
            .map_or(n, |k| i + k + 1),
        Some('0'..='7') => {
            let digits = chars[i + 1..]
                .iter()
                .take(3)
                .take_while(|c| matches!(c, '0'..='7'))
                .count();
            i + 1 + digits
        }
        // A line continuation: the line break is kept
        Some('\n' | '\r') => i + 1,
        Some(_) => i + 2,
        None => i + 1,
    }
}

/// The character the escape `escape` (with its backslash) stands for, when SQL
/// needs it (not whitespace)
fn unescape(escape: &[char]) -> Option<char> {
    let hex = |digits: &[char]| -> Option<char> {
        let text: String = digits.iter().collect();
        u32::from_str_radix(&text, 16).ok().and_then(char::from_u32)
    };
    match escape.get(1)? {
        'x' | 'u' | 'U' => hex(&escape[2..]),
        '0'..='7' => {
            let text: String = escape[1..].iter().collect();
            u32::from_str_radix(&text, 8).ok().and_then(char::from_u32)
        }
        // `\N{name}`: some character; its name doesn't matter to SQL
        'N' => Some('_'),
        'n' | 'r' | 't' | 'a' | 'b' | 'f' | 'v' | '\n' | '\r' => None,
        &c => Some(c),
    }
    .filter(|c| !c.is_whitespace() && *c != '\0')
}

/// Rewrite the driver placeholders in the SQL text `range` of `out` (see the
/// module docs). Identifiers substituted for a list after `IN` are added to
/// `identifiers` (character ranges).
pub(crate) fn rewrite_placeholders(
    out: &mut [char],
    range: std::ops::Range<usize>,
    dialect: SqlDialect,
    identifiers: &mut Vec<(usize, usize)>,
) {
    let placeholder: &[char] = if dialect == SqlDialect::PostgreSQL {
        &['$', '1']
    } else {
        &['?']
    };
    let end = range.end;
    let mut i = range.start;
    while i < end {
        let c = out[i];
        let next = out.get(i + 1).copied().filter(|_| i + 1 < end);
        let prev = (i > range.start).then(|| out[i - 1]);
        let parameter_end = match c {
            // Literals, quoted identifiers and comments are left alone
            '\'' | '"' | '`' => {
                i += 1;
                while i < end && out[i] != c {
                    i += 1;
                }
                i += 1;
                continue;
            }
            '-' if next == Some('-') => {
                while i < end && out[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '%' if next == Some('s') => Some(i + 2),
            '%' if next == Some('(') => out[i..end]
                .iter()
                .position(|&c| c == ')')
                .filter(|&k| out.get(i + k + 1) == Some(&'s'))
                .map(|k| i + k + 2),
            '%' if next == Some('%') => {
                // An escaped `%`
                out[i + 1] = ' ';
                i += 2;
                continue;
            }
            ':' if next.is_some_and(|c| c.is_alphabetic() || c == '_')
                && !prev.is_some_and(|p| is_ident_char(p) || p == ':') =>
            {
                let mut k = i + 1;
                while k < end && is_ident_char(out[k]) {
                    k += 1;
                }
                Some(k)
            }
            _ => None,
        };
        let Some(parameter_end) = parameter_end else {
            i += 1;
            continue;
        };
        let after_in = previous_word(out, range.start, i).eq_ignore_ascii_case("in");
        out[i..parameter_end].fill(' ');
        let width = parameter_end - i;
        if after_in && width >= placeholder.len() + 2 {
            out[i] = '(';
            out[i + 1..=placeholder.len() + i].copy_from_slice(placeholder);
            out[i + placeholder.len() + 1] = ')';
        } else if after_in && i > range.start && out[i - 1].is_whitespace() && width >= 2 {
            // `IN %s`: `IN(_)`, with an identifier whose name diagnostics are
            // dropped when the placeholder is too long for the parentheses
            if placeholder.len() == 1 {
                out[i - 1] = '(';
                out[i] = placeholder[0];
                out[i + 1] = ')';
            } else {
                out[i - 1] = '(';
                out[i] = '_';
                out[i + 1] = ')';
                identifiers.push((i, i + 1));
            }
        } else {
            out[i..i + placeholder.len().min(width)]
                .copy_from_slice(&placeholder[..placeholder.len().min(width)]);
        }
        i = parameter_end;
    }
}

/// The word before index `i` of `out` (from `start`), lowercase or not
fn previous_word(out: &[char], start: usize, i: usize) -> String {
    let mut end = i;
    while end > start && out[end - 1].is_whitespace() {
        end -= 1;
    }
    let mut word_start = end;
    while word_start > start && is_ident_char(out[word_start - 1]) {
        word_start -= 1;
    }
    out[word_start..end].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql_in(source: &str, dialect: SqlDialect) -> String {
        let text = extract(source, DEFAULT_FUNCTIONS, dialect).text;
        assert_eq!(text.chars().count(), source.chars().count());
        assert_eq!(text.lines().count(), source.lines().count());
        text
    }

    fn sql(source: &str) -> String {
        sql_in(source, SqlDialect::PostgreSQL)
    }

    #[test]
    fn extracts_call_arguments() {
        assert_eq!(sql("cur.execute(\"SELECT 1\")"), "            ;SELECT 1; ");
        assert_eq!(
            sql("rows = await conn.fetch('SELECT 1', x)"),
            "                        ;SELECT 1;    "
        );
        assert_eq!(
            sql("db.session.execute(text(\"SELECT 1\"))").trim(),
            ";SELECT 1;"
        );
        assert_eq!(
            sql("cur.execute(\n    \"\"\"\n    SELECT 1\n    \"\"\",\n    (x,),\n)"),
            "            \n    ;  \n    SELECT 1\n      ; \n         \n "
        );
        assert_eq!(sql("cur.execute(r'SELECT 1')").trim(), ";SELECT 1;");
    }

    #[test]
    fn joins_implicitly_concatenated_literals() {
        assert_eq!(
            sql("cur.execute(\"SELECT a \"\n            \"FROM t\")"),
            "            ;SELECT a  \n             FROM t; "
        );
    }

    #[test]
    fn other_strings_are_not_extracted() {
        for source in [
            "x = \"SELECT 1\"",
            "print(\"SELECT 1\")",
            "cur.execute(f\"SELECT {col} FROM t\")",
            "cur.execute(b\"SELECT 1\")",
            "cur.execute(\"SELECT * FROM \" + table)",
            "cur.execute(\"SELECT %s\" % x)",
            "cur.execute(\"SELECT {}\".format(x))",
            "cur.execute(\"SELECT 1\" f\"{x}\")",
            "cur.execute(query, \"SELECT 1\")",
            "cur.execute(\"not sql\")",
            "def execute(\"SELECT 1\")",
            "# cur.execute(\"SELECT 1\")",
            "'''cur.execute(\"SELECT 1\")'''",
            "cur.execute(\"SELECT 1",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
    }

    #[test]
    fn placeholders_become_parameters() {
        assert_eq!(
            sql("cur.execute(\"SELECT 1 WHERE a = %s AND b = %(name)s AND c = :c\")"),
            "            ;SELECT 1 WHERE a = $1 AND b = $1       AND c = $1; "
        );
        assert_eq!(
            sql_in(
                "cur.execute(\"SELECT 1 WHERE a = %s AND c = :c AND d = ?\")",
                SqlDialect::MySQL
            ),
            "            ;SELECT 1 WHERE a = ?  AND c = ?  AND d = ?; "
        );
        // Not in literals, casts or comments; `%%` is a `%`
        assert_eq!(
            sql("cur.execute(\"SELECT a::int, '%s :x', b %% 2 -- %s\")"),
            "            ;SELECT a::int, '%s :x', b %  2 -- %s; "
        );
    }

    #[test]
    fn placeholders_after_in_are_lists() {
        let extracted = extract(
            "cur.execute(\"SELECT 1 WHERE a IN %s AND b IN %(ids)s\")",
            DEFAULT_FUNCTIONS,
            SqlDialect::PostgreSQL,
        );
        assert_eq!(
            extracted.text,
            "            ;SELECT 1 WHERE a IN(_) AND b IN ($1)   ; "
        );
        assert_eq!(extracted.identifiers, [(1, 34, 35)]);
        assert_eq!(
            sql_in("cur.execute(\"SELECT 1 WHERE a IN %s\")", SqlDialect::MySQL),
            "            ;SELECT 1 WHERE a IN(?); "
        );
    }

    #[test]
    fn escapes_become_their_characters() {
        assert_eq!(
            sql("cur.execute(\"SELECT \\\"a\\\",\\n\\tb FROM t\")"),
            "            ;SELECT \"a\",      b FROM t; "
        );
        assert_eq!(
            sql("cur.execute('SELECT \\'a\\' FROM t')"),
            "            ;SELECT 'a'   FROM t; "
        );
    }

    #[test]
    fn functions_are_configurable() {
        let text = extract(
            "db.run_sql(\"SELECT 1\"); cur.execute(\"SELECT 2\")",
            &["run_sql"],
            SqlDialect::PostgreSQL,
        )
        .text;
        assert_eq!(text.trim(), ";SELECT 1;");
    }

    #[test]
    fn directives_in_comments_become_sql_comments() {
        assert_eq!(
            sql("# sqlsift:disable E0002\ncur.execute(\"SELECT 1\")"),
            "--sqlsift:disable E0002\n            ;SELECT 1; "
        );
    }
}
