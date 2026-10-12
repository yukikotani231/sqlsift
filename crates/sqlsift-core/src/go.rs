//! SQL embedded in Go: string literals passed to database calls
//!
//! Go code keeps SQL in string literals, as in
//!
//! ```go
//! rows, err := db.QueryContext(ctx, `
//!     SELECT id, name
//!     FROM users
//!     WHERE id = $1`, id)
//! err = sqlx.GetContext(ctx, db, &user, "SELECT id, name FROM users WHERE email = ?", email)
//! _, err = conn.Exec(ctx, "UPDATE users SET name = @name WHERE id = @id", args)
//! ```
//!
//! which are checked by [`Analyzer::analyze_go`](crate::Analyzer::analyze_go).
//! Guessing which strings are SQL would report on strings that aren't, so only an
//! argument of a call of one of the configured function names is extracted (the
//! name is the last part of the callee: `QueryContext` matches
//! `db.QueryContext(...)` and `r.pool.QueryContext(...)`; see
//! [`DEFAULT_FUNCTIONS`]). For those, the SQL must be where the query goes:
//! after a context (`QueryContext(ctx, ...)`, pgx's `Query(ctx, ...)`), a
//! destination (`Get(&u, ...)`) and the database (`sqlx.Get(db, &u, ...)`), so a
//! string passed as a query argument is never read as SQL; for other configured
//! names, any argument. It is extracted when it
//!
//! - is a string literal (`"..."` or a raw `` `...` ``), or several joined with
//!   `+` (`"SELECT a " +` on one line, `"FROM t"` on the next). A string built
//!   from variables or with `fmt.Sprintf` is skipped: its SQL is only known at
//!   run time.
//! - starts with a statement keyword (`SELECT`, `WITH`, `INSERT`, ...).
//!
//! Driver placeholders become untyped parameters: `$1` (pgx, lib/pq) and `?`
//! (go-sql-driver/mysql, SQLite drivers) as they are, `:name` (sqlx named
//! queries) and `@name` (pgx named arguments) as one. In PostgreSQL SQL, a `?`
//! where a value goes (sqlx queries rebound with `db.Rebind`, `IN (?)` after
//! `sqlx.In`) is a parameter too; the jsonb operators `?`, `?|` and `?&` are
//! left alone.
//!
//! The file is turned into SQL text with the same lines and character columns,
//! as for TypeScript (see [`crate::embedded`]): everything outside the extracted
//! literals becomes spaces (line breaks are kept), the first opening and last
//! closing quotes of a literal become `;`, and `//` and `/* */` comments holding
//! a `sqlsift:disable` directive become `--` comments. Escapes become the
//! character they stand for (`\n` and `\t` a space): the rest of their word
//! moves left, and the following words keep their columns.

use std::path::Path;

use crate::dialect::SqlDialect;
use crate::embedded::{directive, finish, starts_with_statement, Extracted};
use crate::python::{copy_escaped, rewrite_placeholders};

/// File extensions whose SQL is extracted
pub const EXTENSIONS: &[&str] = &["go"];

/// Function names whose SQL argument is extracted by default: the query methods
/// of `database/sql` and pgx (`Query`, `QueryRow`, `Exec`, `Prepare` and their
/// `Context` variants) and of sqlx (`Get`, `Select`, `Queryx`, `NamedExec`, ..., and `sqlx.In`)
pub const DEFAULT_FUNCTIONS: &[&str] = &[
    "Query",
    "QueryContext",
    "QueryRow",
    "QueryRowContext",
    "Exec",
    "ExecContext",
    "Prepare",
    "PrepareContext",
    "Get",
    "GetContext",
    "Select",
    "SelectContext",
    "Queryx",
    "QueryxContext",
    "QueryRowx",
    "QueryRowxContext",
    "MustExec",
    "MustExecContext",
    "NamedExec",
    "NamedExecContext",
    "NamedQuery",
    "NamedQueryContext",
    "Preparex",
    "PreparexContext",
    "In",
];

/// Whether `path` is a Go file (see [`EXTENSIONS`])
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::go::is_go_file;
///
/// assert!(is_go_file(Path::new("internal/store/users.go")));
/// assert!(!is_go_file(Path::new("internal/store/users.py")));
/// ```
pub fn is_go_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether `source` passes SQL to one of `functions` (see the module docs)
///
/// ```
/// use sqlsift_core::go::{has_sql, DEFAULT_FUNCTIONS};
///
/// assert!(has_sql("db.Query(\"SELECT 1\")", DEFAULT_FUNCTIONS));
/// assert!(!has_sql("db.Query(query)", DEFAULT_FUNCTIONS));
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
    /// Index of its opening quote
    open: usize,
    /// Index after its closing quote
    end: usize,
    /// A raw string (backquotes): no escapes
    raw: bool,
    /// Whether it is closed
    closed: bool,
}

impl Literal {
    fn content(&self) -> std::ops::Range<usize> {
        self.open + 1..self.end.saturating_sub(1).max(self.open + 1)
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
    let len = chars.len();
    let mut out: Vec<char> = chars
        .iter()
        .map(|&c| if matches!(c, '\n' | '\r') { c } else { ' ' })
        .collect();
    let mut directives = Vec::new();
    let mut identifiers = Vec::new();

    let mut i = 0;
    while i < len {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                let start = i;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                directives.extend(directive(&chars[start + 2..i], start, Some(i)));
            }
            '/' if next == Some('*') => {
                let start = i;
                i = block_comment_end(&chars, i);
                let first_line_end = (start..i).find(|&k| chars[k] == '\n');
                let text_end = first_line_end.unwrap_or(i);
                let end = first_line_end.is_none().then_some(i);
                directives.extend(directive(&chars[start + 2..text_end], start, end));
            }
            '\'' => i = rune_end(&chars, i),
            '"' | '`' => {
                // A literal, or several joined with `+`
                let mut run = vec![literal_at(&chars, i)];
                let mut j = skip_space(&chars, run[0].end);
                while chars.get(j) == Some(&'+') {
                    let k = skip_space(&chars, j + 1);
                    if !matches!(chars.get(k), Some('"' | '`')) {
                        break;
                    }
                    let literal = literal_at(&chars, k);
                    j = skip_space(&chars, literal.end);
                    run.push(literal);
                }
                i = run.last().map_or(i + 1, |l| l.end);
                let sql = run.iter().all(|l| l.closed)
                    && ends_argument(&chars, i)
                    && is_sql_argument(&chars, run[0].open, functions)
                    && starts_with_statement(&chars, run[0].content().start)
                    && has_two_words(&chars, &run);
                if sql {
                    for literal in &run {
                        let content = literal.content();
                        if literal.raw {
                            for k in content.clone() {
                                // Go drops carriage returns from raw strings
                                out[k] = if chars[k] == '\r' { ' ' } else { chars[k] };
                            }
                        } else {
                            copy_escaped(&chars, &mut out, content.clone());
                        }
                        named_arguments(&mut out, content.clone());
                        if dialect == SqlDialect::PostgreSQL {
                            question_marks(&mut out, content.clone(), &mut identifiers);
                        }
                        rewrite_placeholders(&mut out, content, dialect, &mut identifiers);
                    }
                    if let (Some(first), Some(last)) = (run.first(), run.last()) {
                        out[first.open] = ';';
                        out[last.end - 1] = ';';
                    }
                }
            }
            _ if is_ident_char(c) => {
                while i < len && is_ident_char(chars[i]) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    finish(&chars, out, &directives, identifiers, Vec::new())
}

/// Whether the literals hold more than one word: `c.Query("start")` (gin) and
/// `header.Get("Set-Cookie")` are not SQL
fn has_two_words(chars: &[char], run: &[Literal]) -> bool {
    let mut text = run.iter().flat_map(|l| chars[l.content()].iter());
    text.by_ref().find(|c| !c.is_whitespace());
    text.by_ref().find(|c| c.is_whitespace() || **c == '\\');
    text.any(|c| !c.is_whitespace() && *c != '\\')
}

/// The index after the block comment starting at `i` (Go's don't nest)
fn block_comment_end(chars: &[char], i: usize) -> usize {
    let mut j = i + 2;
    while j < chars.len() {
        if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
            return j + 2;
        }
        j += 1;
    }
    chars.len()
}

/// The index after the rune literal starting at `i` (`'a'`, `'\''`, `'"'`)
fn rune_end(chars: &[char], i: usize) -> usize {
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '\'' => return j + 1,
            '\n' => return j,
            _ => j += 1,
        }
    }
    chars.len()
}

/// The string literal whose opening quote is at `i`
fn literal_at(chars: &[char], i: usize) -> Literal {
    let quote = chars[i];
    let raw = quote == '`';
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' if !raw => j += 2,
            '\n' if !raw => break,
            c if c == quote => {
                return Literal {
                    open: i,
                    end: j + 1,
                    raw,
                    closed: true,
                };
            }
            _ => j += 1,
        }
    }
    Literal {
        open: i,
        end: j.min(chars.len()),
        raw,
        closed: false,
    }
}

/// The index of the next character after whitespace and comments from `i`
fn skip_space(chars: &[char], mut i: usize) -> usize {
    loop {
        match (chars.get(i), chars.get(i + 1)) {
            (Some(c), _) if c.is_whitespace() => i += 1,
            (Some('/'), Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            (Some('/'), Some('*')) => i = block_comment_end(chars, i),
            _ => return i,
        }
    }
}

/// Whether the literal run ending at `end` is a whole argument: followed by `,`
/// or `)`, not by an operator or a method call
fn ends_argument(chars: &[char], end: usize) -> bool {
    matches!(chars.get(skip_space(chars, end)), Some(',' | ')'))
}

/// Whether the literal starting at `i` is an argument of a call of one of
/// `functions`, and no string literal comes before it among the arguments
fn is_sql_argument<S: AsRef<str>>(chars: &[char], i: usize, functions: &[S]) -> bool {
    let Some(open) = call_paren(chars, i) else {
        return false;
    };
    let name_end = skip_space_back(chars, open);
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
    // Not a declaration (`func Query(`, `func (r *Repo) Query(`)
    let before = skip_space_back(chars, start);
    let mut word_start = before;
    let qualified = before > 0 && chars[before - 1] == '.';
    let word_end = if qualified { before - 1 } else { before };
    word_start = word_start.min(word_end);
    while word_start > 0 && is_ident_char(chars[word_start - 1]) {
        word_start -= 1;
    }
    let word: String = chars[word_start..word_end].iter().collect();
    if !qualified && (word == "func" || (before > 0 && chars[before - 1] == ')')) {
        return false;
    }
    let arguments = arguments_before(chars, open + 1, i);
    sql_position(&name, qualified && word == "sqlx", &arguments)
}

/// The arguments of a call from `start` (after its `(`) to `end`, split at the
/// commas outside brackets, trimmed
fn arguments_before(chars: &[char], start: usize, end: usize) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for &c in &chars[start..end] {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    arguments
}

/// Whether the SQL may follow `arguments` in a call of `name` (from `sqlx.` when
/// `package`): a function of [`DEFAULT_FUNCTIONS`] takes it after a context
/// (`QueryContext`, and pgx's `Query(ctx, ...)`), a destination (`Get`,
/// `Select`) and the database (`sqlx.Get(db, ...)`); otherwise a string passed
/// as a query argument (`db.Exec(q, "delete")`) would be read as SQL. Other
/// configured functions take it anywhere.
fn sql_position(name: &str, package: bool, arguments: &[String]) -> bool {
    if !DEFAULT_FUNCTIONS.contains(&name) {
        return true;
    }
    let mut rest = arguments;
    let base = if let Some(base) = name.strip_suffix("Context") {
        let Some((_, after)) = rest.split_first() else {
            return false;
        };
        rest = after;
        base
    } else {
        // pgx: `conn.Query(ctx, sql)`
        if rest.first().is_some_and(|a| is_context(a)) {
            rest = &rest[1..];
        }
        name
    };
    let mut allowed = usize::from(package);
    if matches!(base, "Get" | "Select") {
        allowed += 1;
    }
    rest.len() <= allowed
}

/// Whether a call argument looks like a `context.Context`
fn is_context(argument: &str) -> bool {
    let lower = argument.to_lowercase();
    lower.contains("ctx") || lower.contains("context")
}

/// The index of the `(` of the call whose argument starts at `i`: the arguments
/// before it are skipped back over, keeping brackets balanced; one with a string
/// literal makes it no SQL argument
fn call_paren(chars: &[char], i: usize) -> Option<usize> {
    let mut j = skip_space_back(chars, i);
    let mut depth = 0usize;
    while j > 0 && i - j < 400 {
        j -= 1;
        match chars[j] {
            ')' | ']' | '}' => depth += 1,
            '(' if depth == 0 => return Some(j),
            '(' | '[' | '{' => depth = depth.checked_sub(1)?,
            '"' | '`' | ';' | '\n' if depth == 0 && chars[j] != '\n' => return None,
            '\n' if depth == 0 => {
                // Arguments may span lines, but not a statement boundary: a line
                // ending in `,` or `(` continues the call
                let end = skip_space_back(chars, j);
                if !(end > 0 && matches!(chars[end - 1], ',' | '(')) {
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

/// The index after the last non-whitespace character of `chars[..end]`
fn skip_space_back(chars: &[char], mut end: usize) -> usize {
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    end
}

/// pgx named arguments: `@name` becomes `:name`, a parameter for
/// [`rewrite_placeholders`]. Not in literals, quoted identifiers or comments,
/// nor after a name or `)` (an operator then).
fn named_arguments(out: &mut [char], range: std::ops::Range<usize>) {
    let end = range.end;
    let mut i = range.start;
    let mut prev: Option<char> = None;
    while i < end {
        let c = out[i];
        match c {
            '\'' | '"' => {
                i += 1;
                while i < end && out[i] != c {
                    i += 1;
                }
                i += 1;
                prev = Some(c);
                continue;
            }
            '-' if out.get(i + 1) == Some(&'-') && i + 1 < end => {
                while i < end && out[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '@' if out
                .get(i + 1)
                .is_some_and(|n| i + 1 < end && (n.is_alphabetic() || *n == '_'))
                && !prev.is_some_and(|p| is_ident_char(p) || p == ')' || p == '@') =>
            {
                out[i] = ':';
            }
            _ => {}
        }
        if !c.is_whitespace() {
            prev = Some(c);
        }
        i += 1;
    }
}

/// Words after which a `?` is a value, not the jsonb `?` operator
const VALUE_KEYWORDS: &[&str] = &[
    "and",
    "or",
    "not",
    "in",
    "like",
    "ilike",
    "between",
    "limit",
    "offset",
    "when",
    "then",
    "else",
    "select",
    "values",
    "is",
    "by",
    "return",
    "returning",
];

/// sqlx `?` placeholders in PostgreSQL SQL (rebound to `$n` at run time with
/// `db.Rebind`) become `_`, an identifier whose name diagnostics are dropped:
/// a `?` where a value goes (after an operator, `(`, `,` or a keyword such as
/// `AND`), not the jsonb operator `?` after an expression.
fn question_marks(
    out: &mut [char],
    range: std::ops::Range<usize>,
    identifiers: &mut Vec<(usize, usize)>,
) {
    let end = range.end;
    let mut i = range.start;
    while i < end {
        let c = out[i];
        match c {
            '\'' | '"' => {
                i += 1;
                while i < end && out[i] != c {
                    i += 1;
                }
            }
            '-' if out.get(i + 1) == Some(&'-') && i + 1 < end => {
                while i < end && out[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '?' if is_value_position(out, range.start, i)
                && !out
                    .get(i + 1)
                    .is_some_and(|n| i + 1 < end && matches!(n, '|' | '&')) =>
            {
                out[i] = '_';
                identifiers.push((i, i + 1));
            }
            _ => {}
        }
        i += 1;
    }
}

/// Whether a value goes at index `i` of `out` (from `start`)
fn is_value_position(out: &[char], start: usize, i: usize) -> bool {
    let end = skip_space_back(&out[..i], i).max(start);
    if end == start {
        return true;
    }
    let prev = out[end - 1];
    if !(is_ident_char(prev) || matches!(prev, ')' | ']' | '\'' | '"')) {
        return true;
    }
    let mut word_start = end;
    while word_start > start && is_ident_char(out[word_start - 1]) {
        word_start -= 1;
    }
    let word: String = out[word_start..end]
        .iter()
        .collect::<String>()
        .to_lowercase();
    VALUE_KEYWORDS.contains(&word.as_str())
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
        assert_eq!(sql("db.Query(\"SELECT 1\")"), "         ;SELECT 1; ");
        assert_eq!(
            sql("rows, err := db.QueryContext(ctx, \"SELECT 1\", id)"),
            "                                  ;SELECT 1;     "
        );
        assert_eq!(
            sql("err := sqlx.GetContext(ctx, db, &u, `SELECT 1`)").trim(),
            ";SELECT 1;"
        );
        assert_eq!(
            sql("db.QueryRow(`\n    SELECT 1\n    FROM t`, id)"),
            "            ;\n    SELECT 1\n    FROM t;     "
        );
        assert_eq!(
            sql("r.db.ExecContext(\n\tctx,\n\t\"DELETE FROM t\",\n)").trim(),
            ";DELETE FROM t;"
        );
    }

    #[test]
    fn sql_follows_context_destination_and_database() {
        for source in [
            "conn.Query(ctx, \"SELECT 1\")",
            "conn.Exec(r.Context(), \"SELECT 1\")",
            "db.Get(&u, \"SELECT 1\")",
            "sqlx.Get(db, &u, \"SELECT 1\")",
            "db.SelectContext(ctx, &us, \"SELECT 1\")",
            "sqlx.SelectContext(ctx, db, &us, \"SELECT 1\")",
            "sqlx.NamedExec(db, \"SELECT 1\", x)",
        ] {
            assert_eq!(sql(source).trim(), ";SELECT 1;", "{source}");
        }
    }

    #[test]
    fn joins_literals_concatenated_with_plus() {
        assert_eq!(
            sql("db.Query(\"SELECT a \" +\n\t\"FROM t\")"),
            "         ;SELECT a    \n  FROM t; "
        );
    }

    #[test]
    fn other_strings_are_not_extracted() {
        for source in [
            "q := \"SELECT 1\"",
            "fmt.Println(\"SELECT 1\")",
            "db.Query(fmt.Sprintf(\"SELECT %s FROM t\", col))",
            "db.Query(\"SELECT * FROM \" + table)",
            "db.Query(q, \"SELECT 1\")",
            "db.Exec(q, \"delete\", \"update x\")",
            "db.ExecContext(ctx, q, \"select 1\")",
            "db.Exec(\"not sql\")",
            "func Query(\"SELECT 1\")",
            "func (r *Repo) Query(\"SELECT 1\")",
            "// db.Query(\"SELECT 1\")",
            "/* db.Query(\"SELECT 1\") */",
            "x := '\"'; db.Query(\"SELECT 1",
            "db.Query(\"SELECT 1\".x)",
            "db.Query(\"SELECT 1\" + suffix)",
            "c.Query(\"start\")",
            "r.Header.Get(\"Set-Cookie\")",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
    }

    #[test]
    fn placeholders_become_parameters() {
        assert_eq!(
            sql("db.NamedExec(\"UPDATE t SET a = :a WHERE id = @id AND b = $2\", x)"),
            "             ;UPDATE t SET a = $1 WHERE id = $1  AND b = $2;    "
        );
        assert_eq!(
            sql_in(
                "db.Select(&xs, \"SELECT 1 WHERE a = ? AND b IN (?)\", a, b)",
                SqlDialect::MySQL
            )
            .trim(),
            ";SELECT 1 WHERE a = ? AND b IN (?);"
        );
        // sqlx `?` before `db.Rebind`, but not the jsonb operators
        assert_eq!(
            sql("db.Select(&xs, \"SELECT 1 WHERE a = ? AND b IN (?, ?) AND c ? 'k' AND d ?| e\", a)")
                .trim(),
            ";SELECT 1 WHERE a = _ AND b IN (_, _) AND c ? 'k' AND d ?| e;"
        );
        // Not casts, operators, literals or comments
        assert_eq!(
            sql("db.Query(\"SELECT a::int, b @> c, '@x :y' -- @z\")").trim(),
            ";SELECT a::int, b @> c, '@x :y' -- @z;"
        );
    }

    #[test]
    fn escapes_become_their_characters() {
        assert_eq!(
            sql("db.Query(\"SELECT \\\"a\\\",\\n\\tb FROM t\")"),
            "         ;SELECT \"a\",      b FROM t; "
        );
    }

    #[test]
    fn directives_in_comments() {
        let text = sql("// sqlsift:disable E0002\ndb.Query(\"SELECT 1\")");
        assert!(text.starts_with("-- sqlsift:disable E0002"), "{text}");
    }

    #[test]
    fn functions_are_configurable() {
        let text = extract(
            "store.RunSQL(\"SELECT 1\"); db.Query(\"SELECT 2\")",
            &["RunSQL"],
            SqlDialect::PostgreSQL,
        )
        .text;
        assert_eq!(text.trim(), ";SELECT 1;");
    }
}
