//! SQL embedded in Rust: sqlx's query macros and functions
//!
//! Queries written as string literals in Rust code, such as
//!
//! ```rust,ignore
//! let user = sqlx::query_as!(User, "SELECT id, name FROM users WHERE id = $1", id)
//!     .fetch_one(&pool)
//!     .await?;
//! let count: i64 = sqlx::query_scalar(r#"SELECT count(*) FROM "users""#)
//!     .fetch_one(&pool)
//!     .await?;
//! ```
//!
//! are checked by [`Analyzer::analyze_rust`](crate::Analyzer::analyze_rust). A
//! string literal is extracted when it is
//!
//! - the first argument of a macro whose name (the last segment of its path) is
//!   one of the configured macros: `query!("...")`, `sqlx::query!("...")`; or the
//!   second, after a type, as in `query_as!(User, "...")`;
//! - the first argument of a call of one of them as a function of the `sqlx`
//!   crate: `sqlx::query("...")`, `sqlx::query_as::<_, User>("...")`. Unqualified
//!   calls (`query("...")`) are not extracted, since any function may be named so.
//!
//! A string that doesn't start with a statement keyword (`SELECT`, `WITH`,
//! `INSERT`, ...) is not SQL, and is not extracted: another crate's `query!`
//! macro may take something else.
//!
//! The SQL keeps sqlx's own bind parameters (`$1` for PostgreSQL, `?` for MySQL
//! and SQLite). `query_file!` needs nothing here: its `.sql` files are checked
//! directly.
//!
//! The file is turned into SQL text with the same lines and character columns,
//! as for TypeScript (see [`crate::embedded`]): everything outside the extracted
//! literals becomes spaces (line breaks are kept), the literals' quotes become
//! `;`, a raw string's `r` and `#`s become spaces, and `//` and `/* */` comments
//! holding a `sqlsift:disable` directive become `--` comments. Escapes become the
//! character they stand for (`\n` and `\t` a space): the rest of their word moves
//! left, and the following words keep their columns.

use std::path::Path;

use crate::embedded::{directive, finish, starts_with_statement, Extracted};

/// File extensions whose SQL is extracted from sqlx macros and functions
pub const EXTENSIONS: &[&str] = &["rs"];

/// sqlx macros (and `sqlx::` functions) whose SQL is extracted by default
pub const DEFAULT_MACROS: &[&str] = &[
    "query",
    "query_as",
    "query_scalar",
    "query_unchecked",
    "query_as_unchecked",
    "query_scalar_unchecked",
];

/// Whether `path` is a Rust file (see [`EXTENSIONS`])
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::sqlx::is_rust_file;
///
/// assert!(is_rust_file(Path::new("src/db.rs")));
/// assert!(!is_rust_file(Path::new("src/db.ts")));
/// ```
pub fn is_rust_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Extract the SQL of the string literals passed to one of `macros` (see the
/// module docs)
pub(crate) fn extract<S: AsRef<str>>(source: &str, macros: &[S]) -> Extracted {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut out: Vec<char> = chars
        .iter()
        .map(|&c| if matches!(c, '\n' | '\r') { c } else { ' ' })
        .collect();
    let mut directives = Vec::new();

    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                let start = i;
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                directives.extend(directive(&chars[start + 2..i], start, Some(i)));
            }
            '/' if next == Some('*') => {
                // Block comments nest
                let start = i;
                let mut depth = 0usize;
                while i < n {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                let first_line_end = (start..i).find(|&k| chars[k] == '\n');
                let text_end = first_line_end.unwrap_or(i);
                let end = first_line_end.is_none().then_some(i);
                directives.extend(directive(&chars[start + 2..text_end], start, end));
            }
            '\'' => i = skip_char_or_lifetime(&chars, i),
            '"' => {
                let sql =
                    is_sql_argument(&chars, i, macros) && starts_with_statement(&chars, i + 1);
                i = string(&chars, &mut out, i, sql);
            }
            _ if is_ident_char(c) => {
                let start = i;
                while i < n && is_ident_char(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if matches!(word.as_str(), "r" | "br" | "cr") {
                    // A raw string (`r"..."`, `r#"..."#`), or a raw identifier
                    let hashes = chars[i..].iter().take_while(|&&c| c == '#').count();
                    if chars.get(i + hashes) == Some(&'"') {
                        let sql = word == "r"
                            && is_sql_argument(&chars, start, macros)
                            && starts_with_statement(&chars, i + hashes + 1);
                        i = raw_string(&chars, &mut out, i + hashes, hashes, sql);
                    }
                } else if matches!(word.as_str(), "b" | "c") && chars.get(i) == Some(&'"') {
                    // A byte or C string
                    i = string(&chars, &mut out, i, false);
                }
            }
            _ => i += 1,
        }
    }

    finish(&chars, out, &directives, Vec::new(), Vec::new())
}

/// The index after the character literal or lifetime starting at `i`
fn skip_char_or_lifetime(chars: &[char], i: usize) -> usize {
    match (chars.get(i + 1), chars.get(i + 2)) {
        // An escape: `'\n'`, `'\''`, `'\u{e9}'`
        (Some('\\'), _) => {
            let mut j = i + 3;
            while j < chars.len() && chars[j] != '\'' && chars[j] != '\n' {
                j += 1;
            }
            j + 1
        }
        (Some(_), Some('\'')) => i + 3,
        // A lifetime or label: `'a`, `'static`
        _ => i + 1,
    }
}

/// Copy the string literal whose opening quote is at `i` to `out` when `sql`, and
/// return the index after it. Quotes become `;` and escapes the character they
/// stand for, written right after the text before them so the word they are in
/// stays whole; the spaces this saves go at the next whitespace, so that the next
/// word is in its place again.
fn string(chars: &[char], out: &mut [char], open: usize, sql: bool) -> usize {
    let len = chars.len();
    if sql {
        out[open] = ';';
    }
    // Where the next character is written
    let mut at = open + 1;
    let mut j = open + 1;
    while j < len {
        match chars[j] {
            '"' => {
                if sql {
                    out[j] = ';';
                }
                return j + 1;
            }
            '\\' => {
                let end = escape_end(chars, j);
                match unescape(&chars[j..end]) {
                    Some(c) => {
                        if sql {
                            out[at] = c;
                        }
                        at += 1;
                    }
                    None => at = end,
                }
                j = end;
            }
            c if c.is_whitespace() => {
                j += 1;
                at = j;
            }
            c => {
                if sql {
                    out[at] = c;
                }
                at += 1;
                j += 1;
            }
        }
    }
    len
}

/// The index after the escape starting with the backslash at `i`
fn escape_end(chars: &[char], i: usize) -> usize {
    match chars.get(i + 1) {
        Some('x') => (i + 4).min(chars.len()),
        Some('u') => chars[i..]
            .iter()
            .position(|&c| c == '}')
            .map_or(chars.len(), |k| i + k + 1),
        // A line continuation: the line break is kept, the next line's indent is
        // whitespace anyway
        Some('\n' | '\r') => i + 1,
        Some(_) => i + 2,
        None => i + 1,
    }
}

/// The character the escape `escape` (with its backslash) stands for, when SQL
/// needs it (not whitespace)
fn unescape(escape: &[char]) -> Option<char> {
    match escape.get(1)? {
        'x' => {
            let hex: String = escape[2..].iter().collect();
            u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
        }
        'u' => {
            let hex: String = escape[2..]
                .iter()
                .filter(|c| c.is_ascii_hexdigit())
                .collect();
            u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
        }
        'n' | 'r' | 't' | '0' | '\n' | '\r' => None,
        &c => Some(c),
    }
    .filter(|c| !c.is_whitespace() && *c != '\0')
}

/// Copy the raw string whose opening quote is at `i`, closed by a quote and
/// `hashes` `#`s, to `out` when `sql` (quotes become `;`), and return the index
/// after it
fn raw_string(chars: &[char], out: &mut [char], i: usize, hashes: usize, sql: bool) -> usize {
    let n = chars.len();
    if sql {
        out[i] = ';';
    }
    let mut j = i + 1;
    while j < n {
        if chars[j] == '"'
            && chars[j + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == '#')
                .count()
                == hashes
        {
            if sql {
                out[j] = ';';
            }
            return j + 1 + hashes;
        }
        if sql {
            out[j] = chars[j];
        }
        j += 1;
    }
    n
}

/// Whether the string literal starting at `i` (its quote or raw prefix) is the
/// SQL of a call of one of `macros` (see the module docs)
fn is_sql_argument<S: AsRef<str>>(chars: &[char], i: usize, macros: &[S]) -> bool {
    let is_macro = |name: &str| macros.iter().any(|m| m.as_ref() == name);
    let mut end = skip_space_back(chars, i);
    if end > 0 && chars[end - 1] == ',' {
        // `query_as!(User, "...")`: the type before is a path
        let path_end = skip_space_back(chars, end - 1);
        let mut start = path_end;
        while start > 0 && (is_ident_char(chars[start - 1]) || chars[start - 1] == ':') {
            start -= 1;
        }
        if start == path_end {
            return false;
        }
        end = skip_space_back(chars, start);
        if end == 0 || !matches!(chars[end - 1], '(' | '[' | '{') {
            return false;
        }
        let bang = skip_space_back(chars, end - 1);
        if bang == 0 || chars[bang - 1] != '!' {
            return false;
        }
        let (name, _) = ident_before(chars, skip_space_back(chars, bang - 1));
        return name.as_deref().is_some_and(is_macro);
    }
    if end == 0 || !matches!(chars[end - 1], '(' | '[' | '{') {
        return false;
    }
    let delimiter = chars[end - 1];
    end = skip_space_back(chars, end - 1);
    // A macro: `query!(`, `sqlx::query! [`
    if end > 0 && chars[end - 1] == '!' {
        let (name, _) = ident_before(chars, skip_space_back(chars, end - 1));
        return name.as_deref().is_some_and(is_macro);
    }
    if delimiter != '(' {
        return false;
    }
    // A function of the sqlx crate: `sqlx::query(`, `sqlx::query_as::<_, T>(`
    if end > 0 && chars[end - 1] == '>' {
        let Some(open) = turbofish_start(chars, end) else {
            return false;
        };
        end = skip_space_back(chars, open);
        if !(end >= 2 && chars[end - 1] == ':' && chars[end - 2] == ':') {
            return false;
        }
        end = skip_space_back(chars, end - 2);
    }
    let (name, start) = ident_before(chars, end);
    if !name.as_deref().is_some_and(is_macro) {
        return false;
    }
    let colons = skip_space_back(chars, start);
    if !(colons >= 2 && chars[colons - 1] == ':' && chars[colons - 2] == ':') {
        return false;
    }
    let (krate, _) = ident_before(chars, skip_space_back(chars, colons - 2));
    krate.as_deref() == Some("sqlx")
}

/// The identifier ending at `end`, and the index it starts at
fn ident_before(chars: &[char], end: usize) -> (Option<String>, usize) {
    let mut start = end;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    let name = (start < end).then(|| chars[start..end].iter().collect());
    (name, start)
}

/// The index of the `<` of the turbofish whose `>` is at `end - 1`
fn turbofish_start(chars: &[char], end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = end;
    while j > 0 && end - j < 500 {
        j -= 1;
        match chars[j] {
            '>' => depth += 1,
            '<' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            ';' | '{' | '}' => return None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sql(source: &str) -> String {
        let text = extract(source, DEFAULT_MACROS).text;
        assert_eq!(text.chars().count(), source.chars().count());
        assert_eq!(text.lines().count(), source.lines().count());
        text
    }

    #[test]
    fn extracts_macro_arguments() {
        assert_eq!(
            sql("let r = query!(\"SELECT 1\", x);"),
            "               ;SELECT 1;     "
        );
        assert_eq!(
            sql("sqlx::query_as!(User, \"SELECT 1\")"),
            "                      ;SELECT 1; "
        );
        assert_eq!(
            sql("sqlx::query_scalar_unchecked! [ \"SELECT 1\" ]"),
            "                                ;SELECT 1;  "
        );
        assert_eq!(
            sql("sqlx::query_as!(\n    models::User,\n    \"SELECT 1\"\n)").trim(),
            ";SELECT 1;"
        );
    }

    #[test]
    fn extracts_sqlx_function_arguments() {
        for source in [
            "sqlx::query(\"SELECT 1\")",
            "sqlx::query_as::<_, User>(\"SELECT 1\")",
            "sqlx::query_as::<Postgres, (i64, Option<String>)>(\"SELECT 1\")",
            "sqlx::query_scalar (\n  \"SELECT 1\")",
        ] {
            assert_eq!(sql(source).trim(), ";SELECT 1;", "{source}");
        }
    }

    #[test]
    fn other_strings_are_not_extracted() {
        for source in [
            "query(\"SELECT 1\")",
            "other::query(\"SELECT 1\")",
            "println!(\"SELECT 1\")",
            "query_file!(\"queries/a.sql\")",
            "query!(f(\"SELECT 1\"))",
            "query_as!(User, x, \"SELECT 1\")",
            "let s = \"SELECT 1\";",
            "// query!(\"SELECT 1\")",
            "/* /* */ query!(\"SELECT 1\") */",
            "'\"'; query; (\"SELECT 1\")",
            "b\"x\"; query!(b\"SELECT 1\")",
            "query!(\"{ user { id } }\")",
            "query!(r#\"query { user }\"#)",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
    }

    #[test]
    fn raw_strings_keep_their_text() {
        assert_eq!(
            sql("query!(r#\"SELECT \"a\" FROM t\"#)"),
            "         ;SELECT \"a\" FROM t;  "
        );
        assert_eq!(sql("query!(r\"SELECT '\\n'\")"), "        ;SELECT '\\n'; ");
        // A raw identifier isn't a string
        assert_eq!(
            sql("let r#type = 1; query!(\"SELECT 1\")").trim(),
            ";SELECT 1;"
        );
    }

    #[test]
    fn escapes_become_their_characters() {
        assert_eq!(
            sql("query!(\"SELECT \\\"a\\\",\\n\\tb FROM t\")"),
            "       ;SELECT \"a\",      b FROM t; "
        );
        assert_eq!(
            sql("query!(\"SELECT '\\x41' \\\n   FROM t\")"),
            "       ;SELECT 'A'     \n   FROM t; "
        );
        assert_eq!(sql("query!(\"SELECT 'é'\")"), "       ;SELECT 'é'; ");
    }

    #[test]
    fn lifetimes_and_chars_are_skipped() {
        assert_eq!(
            sql("fn f<'a>(x: &'a str) { let c = '\"'; query!(\"SELECT 1\") }").trim(),
            ";SELECT 1;"
        );
        assert_eq!(
            sql("let c = '\\''; query!(\"SELECT 1\")").trim(),
            ";SELECT 1;"
        );
    }

    #[test]
    fn macros_are_configurable() {
        let text = extract(
            "my_query!(\"SELECT 1\"); query!(\"SELECT 2\")",
            &["my_query"],
        )
        .text;
        assert_eq!(text.trim(), ";SELECT 1;");
    }

    #[test]
    fn directives_in_comments_become_sql_comments() {
        assert_eq!(
            sql("// sqlsift:disable E0002\nquery!(\"SELECT 1\")"),
            "-- sqlsift:disable E0002\n       ;SELECT 1; "
        );
    }
}
