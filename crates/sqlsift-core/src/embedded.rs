//! SQL embedded in TypeScript and JavaScript
//!
//! Queries written as tagged template literals, such as
//!
//! ```ts
//! const posts = await sql`SELECT id, title FROM posts WHERE author_id = ${authorId}`;
//! const users = await prisma.$queryRaw<User[]>`SELECT * FROM users`;
//! ```
//!
//! are checked by [`Analyzer::analyze_embedded`](crate::Analyzer::analyze_embedded).
//! A template is extracted when the last identifier of its tag expression (`sql` in
//! `sql`, `db.sql` and `Prisma.sql`, `$queryRaw` in `prisma.$queryRaw<T>`) is one
//! of the configured tags. Extraction uses a small lexer that knows about comments,
//! string literals, regular expression literals and nested templates, and turns the
//! file into SQL text with the same lines and character columns:
//!
//! - Everything outside the extracted templates becomes spaces (line breaks are
//!   kept), and each template's backticks become `;`, so every template is a
//!   statement of its own.
//! - `${expr}` becomes an untyped placeholder (`$1`, or `?` for MySQL and SQLite),
//!   `($1)` after `IN`, and an identifier as long as the interpolation where a table
//!   name is expected (after FROM, JOIN, INTO, UPDATE, TABLE); name diagnostics
//!   reported on such an identifier are dropped, as for psql's `:"var"`.
//! - Templates nested in an extracted template's `${...}` are not extracted (they
//!   usually are query fragments).
//!
//! Because each character is replaced by one character, diagnostics found in the
//! extracted text have the right line and column in the original file.

use std::path::Path;

use crate::dialect::SqlDialect;
use crate::psql::TABLE_KEYWORDS;

/// File extensions whose SQL is extracted from tagged template literals
pub const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts"];

/// Template tags extracted by default
pub const DEFAULT_TAGS: &[&str] = &["sql"];

/// Whether `path` is a TypeScript or JavaScript file (see [`EXTENSIONS`])
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::embedded::is_embedded_sql_file;
///
/// assert!(is_embedded_sql_file(Path::new("src/db/posts.ts")));
/// assert!(!is_embedded_sql_file(Path::new("queries/posts.sql")));
/// ```
pub fn is_embedded_sql_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// SQL extracted from a source file
pub(crate) struct Extracted {
    /// The SQL text (same lines and character columns as the source)
    pub text: String,
    /// Identifiers substituted for interpolations: (line, first column, end
    /// column), 1-indexed and end-exclusive
    pub identifiers: Vec<(usize, usize, usize)>,
}

/// Lexer state: what an open backtick or `${` belongs to
enum Frame {
    /// A template literal; `sql` when its text is extracted
    Template { sql: bool },
    /// `${ ... }` in a template literal: the depth of `{` opened in it, the index of
    /// its `$` and whether the template is extracted
    Interpolation {
        depth: usize,
        start: usize,
        sql: bool,
    },
}

/// The previous token in code, to tell a regular expression from a division
enum Previous {
    /// Nothing, or an operator or punctuation: a `/` starts a regular expression
    Operator,
    /// An identifier or keyword
    Word(String),
    /// A literal or closing bracket: a `/` is a division
    Value,
}

/// Keywords after which a `/` starts a regular expression
const REGEX_KEYWORDS: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Extract the SQL of the templates tagged with one of `tags` (see the module docs)
pub(crate) fn extract<S: AsRef<str>>(source: &str, tags: &[S], dialect: SqlDialect) -> Extracted {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut out: Vec<char> = chars
        .iter()
        .map(|&c| if matches!(c, '\n' | '\r') { c } else { ' ' })
        .collect();
    // Substituted identifiers as character index ranges
    let mut identifiers = Vec::new();

    let mut stack: Vec<Frame> = Vec::new();
    let mut previous = Previous::Operator;
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();

        // Inside a template literal
        if let Some(&Frame::Template { sql }) = stack.last() {
            match c {
                '\\' => {
                    // An escape: keep the escaped character
                    if sql && i + 1 < n {
                        out[i + 1] = chars[i + 1];
                    }
                    i += 2;
                }
                '`' => {
                    stack.pop();
                    if sql {
                        out[i] = ';';
                    }
                    previous = Previous::Value;
                    i += 1;
                }
                '$' if next == Some('{') => {
                    stack.push(Frame::Interpolation {
                        depth: 0,
                        start: i,
                        sql,
                    });
                    previous = Previous::Operator;
                    i += 2;
                }
                _ => {
                    if sql {
                        out[i] = c;
                    }
                    i += 1;
                }
            }
            continue;
        }

        // In code: at the top level or in a `${ ... }`
        match c {
            '/' if next == Some('/') => {
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                i += 2;
                while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            '\'' | '"' => {
                i = skip_string(&chars, i);
                previous = Previous::Value;
            }
            '`' => {
                let in_sql = stack
                    .iter()
                    .any(|f| matches!(f, Frame::Template { sql: true }));
                let sql = !in_sql && is_tagged(&chars, i, tags);
                if sql {
                    // Ends a statement a `--` comment in the previous template hid
                    // the end of
                    out[i] = ';';
                }
                stack.push(Frame::Template { sql });
                i += 1;
            }
            '{' => {
                if let Some(Frame::Interpolation { depth, .. }) = stack.last_mut() {
                    *depth += 1;
                }
                previous = Previous::Operator;
                i += 1;
            }
            '}' => {
                match stack.last_mut() {
                    Some(Frame::Interpolation {
                        depth: 0,
                        start,
                        sql,
                    }) => {
                        let (start, sql) = (*start, *sql);
                        stack.pop();
                        if sql {
                            let identifier = placeholder(&chars, &mut out, start, i + 1, dialect);
                            identifiers.extend(identifier);
                        }
                    }
                    Some(Frame::Interpolation { depth, .. }) => *depth -= 1,
                    _ => {}
                }
                previous = Previous::Operator;
                i += 1;
            }
            '/' if regex_allowed(&previous) => {
                // Not a regular expression if it doesn't end on this line
                let end = skip_regex(&chars, i);
                previous = if end.is_some() {
                    Previous::Value
                } else {
                    Previous::Operator
                };
                i = end.unwrap_or(i + 1);
            }
            _ if c.is_ascii_digit() => {
                while i < n && (chars[i].is_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                previous = Previous::Value;
            }
            _ if is_ident_char(c) => {
                let start = i;
                while i < n && is_ident_char(chars[i]) {
                    i += 1;
                }
                previous = Previous::Word(chars[start..i].iter().collect());
            }
            _ if c.is_whitespace() => i += 1,
            ')' | ']' => {
                previous = Previous::Value;
                i += 1;
            }
            _ => {
                previous = Previous::Operator;
                i += 1;
            }
        }
    }

    // Character indexes to line / column
    let mut line_starts = vec![0];
    line_starts.extend(
        chars
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c == '\n')
            .map(|(k, _)| k + 1),
    );
    let position = |k: usize| {
        let line = line_starts.partition_point(|&s| s <= k);
        (line, k - line_starts[line - 1] + 1)
    };
    let identifiers = identifiers
        .into_iter()
        .map(|(start, end)| {
            let (line, first) = position(start);
            (line, first, first + (end - start))
        })
        .collect();

    Extracted {
        text: out.into_iter().collect(),
        identifiers,
    }
}

/// Whether the tag of the template whose backtick is at `i` is one of `tags`: the
/// last identifier before it, skipping type arguments (`$queryRaw<User[]>`)
fn is_tagged<S: AsRef<str>>(chars: &[char], i: usize, tags: &[S]) -> bool {
    let skip_space = |mut j: usize| {
        while j > 0 && chars[j - 1].is_whitespace() {
            j -= 1;
        }
        j
    };
    let mut end = skip_space(i);
    if end > 0 && chars[end - 1] == '>' {
        // Type arguments, on the same line
        let mut depth = 0;
        let mut j = end;
        loop {
            if j == 0 || matches!(chars[j - 1], '\n' | ';' | '`') {
                return false;
            }
            j -= 1;
            match chars[j] {
                '>' => depth += 1,
                '<' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        end = skip_space(j);
    }
    let mut start = end;
    while start > 0 && is_ident_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return false;
    }
    let tag: String = chars[start..end].iter().collect();
    tags.iter().any(|t| t.as_ref() == tag)
}

/// The index after the string literal starting at `i` (or the end of its line, if
/// it isn't closed there)
fn skip_string(chars: &[char], i: usize) -> usize {
    let quote = chars[i];
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '\n' => return j,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    chars.len()
}

/// The index after the regular expression literal starting at `i`, or `None` if
/// there isn't one on this line
fn skip_regex(chars: &[char], i: usize) -> Option<usize> {
    let mut in_class = false;
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 1,
            '\n' => return None,
            '[' => in_class = true,
            ']' => in_class = false,
            '/' if !in_class => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

fn regex_allowed(previous: &Previous) -> bool {
    match previous {
        Previous::Operator => true,
        Previous::Word(word) => REGEX_KEYWORDS.contains(&word.as_str()),
        Previous::Value => false,
    }
}

/// Write the SQL for the interpolation `chars[start..end]` (`${...}`) to `out`,
/// returning the character range of an identifier substituted for it
fn placeholder(
    chars: &[char],
    out: &mut [char],
    start: usize,
    end: usize,
    dialect: SqlDialect,
) -> Option<(usize, usize)> {
    // The SQL word before the interpolation
    let mut word_end = start;
    while word_end > 0 && out[word_end - 1].is_whitespace() {
        word_end -= 1;
    }
    let mut word_start = word_end;
    while word_start > 0
        && (out[word_start - 1].is_ascii_alphanumeric() || out[word_start - 1] == '_')
    {
        word_start -= 1;
    }
    let word: String = out[word_start..word_end].iter().collect();
    let is_word = |k: &&str| word.eq_ignore_ascii_case(k);

    let first_line_end = (start..end).find(|&k| chars[k] == '\n').unwrap_or(end);
    if TABLE_KEYWORDS.iter().any(is_word) {
        // An identifier as long as the interpolation's first line: `__table_`
        for k in start..first_line_end {
            let c = chars[k];
            out[k] = if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            };
        }
        // Including a qualified name it starts (`${schema}.users`)
        let mut name_end = first_line_end;
        if first_line_end == end {
            while name_end < chars.len()
                && (chars[name_end].is_alphanumeric() || matches!(chars[name_end], '_' | '.' | '"'))
            {
                name_end += 1;
            }
        }
        return Some((start, name_end));
    }

    let parameter = match dialect {
        SqlDialect::PostgreSQL => "$1",
        SqlDialect::MySQL | SqlDialect::SQLite => "?",
    };
    // A list after IN (`IN ${ids}`) needs parentheses
    let text = if word.eq_ignore_ascii_case("in") && first_line_end - start >= parameter.len() + 2 {
        format!("({parameter})")
    } else {
        parameter.to_string()
    };
    for (k, c) in text.chars().enumerate() {
        out[start + k] = c;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql(source: &str) -> String {
        let text = extract(source, DEFAULT_TAGS, SqlDialect::PostgreSQL).text;
        assert_eq!(text.chars().count(), source.chars().count());
        assert_eq!(text.lines().count(), source.lines().count());
        text
    }

    #[test]
    fn extracts_tagged_templates() {
        assert_eq!(sql("const q = sql`SELECT 1`;"), "             ;SELECT 1; ");
        assert_eq!(sql("db.sql`SELECT 1`"), "      ;SELECT 1;");
        assert_eq!(sql("x = other`SELECT 1`"), "                   ");
        assert_eq!(sql("x = `SELECT 1`"), "              ");
    }

    #[test]
    fn matches_the_last_identifier_of_the_tag() {
        let tags = ["$queryRaw"];
        let extract = |s: &str| extract(s, &tags, SqlDialect::PostgreSQL).text;
        assert_eq!(
            extract("prisma.$queryRaw`SELECT 1`"),
            "                ;SELECT 1;"
        );
        assert_eq!(
            extract("prisma.$queryRaw<User[]>`SELECT 1`"),
            "                        ;SELECT 1;"
        );
        assert_eq!(
            extract("prisma.queryRaw`SELECT 1`"),
            "                         "
        );
    }

    #[test]
    fn interpolations_become_placeholders() {
        assert_eq!(
            sql("sql`SELECT * FROM t WHERE a = ${x.y} AND b IN ${ids}`"),
            "   ;SELECT * FROM t WHERE a = $1     AND b IN ($1)  ;"
        );
        let mysql = extract("sql`WHERE a = ${x}`", DEFAULT_TAGS, SqlDialect::MySQL).text;
        assert_eq!(mysql, "   ;WHERE a = ?   ;");
    }

    #[test]
    fn interpolated_table_names_become_identifiers() {
        let extracted = extract(
            "sql`SELECT 1\nFROM ${table} JOIN ${s}.users`",
            DEFAULT_TAGS,
            SqlDialect::PostgreSQL,
        );
        assert_eq!(
            extracted.text,
            "   ;SELECT 1\nFROM __table_ JOIN __s_.users;"
        );
        assert_eq!(extracted.identifiers, vec![(2, 6, 14), (2, 20, 30)]);
    }

    #[test]
    fn interpolations_may_nest_braces_and_templates() {
        assert_eq!(
            sql("sql`SELECT ${f({ a: `x${1}` })} FROM t`"),
            "   ;SELECT $1                   FROM t;"
        );
        // A template nested in an extracted one's interpolation is a fragment
        assert_eq!(
            sql("sql`SELECT 1 ${sql`AND x`}`"),
            "   ;SELECT 1 $1           ;"
        );
    }

    #[test]
    fn skips_comments_strings_and_regexes() {
        for source in [
            "// sql`SELECT 1`",
            "/* sql`SELECT 1` */",
            "'sql`SELECT 1`'",
            "\"it's sql`SELECT 1`\"",
            "x = /sql`[`]/g",
            "if (a) /`/.test(s)",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
        // A division isn't a regular expression
        assert_eq!(sql("a / 2 / sql`SELECT 1`").trim(), ";SELECT 1;");
    }

    #[test]
    fn keeps_escaped_characters_and_multibyte_columns() {
        assert_eq!(sql("sql`SELECT \\`a\\``"), "   ;SELECT  `a `;");
        assert_eq!(sql("'é'; sql`SELECT 'ü'`"), "        ;SELECT 'ü';");
    }

    #[test]
    fn recognizes_file_extensions() {
        for name in ["a.ts", "a.tsx", "a.js", "a.jsx", "a.mts", "a.cts", "A.TS"] {
            assert!(is_embedded_sql_file(Path::new(name)), "{name}");
        }
        for name in ["a.sql", "a", "ts"] {
            assert!(!is_embedded_sql_file(Path::new(name)), "{name}");
        }
    }
}
