//! SQL embedded in TypeScript and JavaScript
//!
//! Queries written as tagged template literals, such as
//!
//! ```ts
//! const posts = await sql`SELECT id, title FROM posts WHERE author_id = ${authorId}`;
//! const users = await prisma.$queryRaw<User[]>`SELECT * FROM users`;
//! ```
//!
//! are checked by [`Analyzer::analyze_embedded`](crate::Analyzer::analyze_embedded)
//! (and, for the `<script>` blocks of Vue and Svelte components, by
//! [`Analyzer::analyze_embedded_component`](crate::Analyzer::analyze_embedded_component)).
//! A template is extracted when its tag expression is a chain of member accesses and
//! calls whose last or first identifier is one of the configured tags: `sql` matches
//! `sql`, `db.sql` and `Prisma.sql` (last identifier) as well as Slonik's
//! `sql.unsafe`, `sql.type(schema)` and `sql.typeAlias('id')` (first identifier),
//! and `$queryRaw` matches `prisma.$queryRaw<T>`. Templates that don't start with a
//! statement keyword (`SELECT`, `WITH`, `INSERT`, ...) are query fragments, such as
//! `` sql`AND published` ``, and are not extracted.
//!
//! Extraction uses a small lexer that knows about comments, string literals,
//! regular expression literals and nested templates, and turns the file into SQL
//! text with the same lines and character columns:
//!
//! - Everything outside the extracted templates becomes spaces (line breaks are
//!   kept), and each template's backticks become `;`, so every template is a
//!   statement of its own. `//` and `/* */` comments holding a `sqlsift:disable`
//!   directive become `--` comments.
//! - `${expr}` becomes an untyped placeholder (`$1`, or `?` for MySQL and SQLite),
//!   `($1)` after `IN`, and an identifier as long as the interpolation where a table
//!   name is expected (after FROM, JOIN, INTO, UPDATE, TABLE); name diagnostics
//!   reported on such an identifier are dropped, as for psql's `:"var"`.
//! - postgres.js helpers are rows and assignments of unknown columns:
//!   `INSERT INTO t ${sql(row)}` becomes `INSERT INTO t SELECT*FROM <identifier>`,
//!   `VALUES ${sql(rows)}` becomes `SELECT *FROM <identifier>`, and
//!   `SET ${sql(patch)}` becomes `SET <identifier>=$1`.
//! - `${expr}` after a value or a name (`WHERE a = 1 ${filter}`,
//!   `FROM users ${where}`) is a query fragment between clauses, and is dropped.
//! - Templates nested in an extracted template's `${...}` are not extracted (they
//!   usually are query fragments).
//!
//! Because each character is replaced by one character, diagnostics found in the
//! extracted text have the right line and column in the original file.

use std::path::Path;

use crate::dialect::SqlDialect;
use crate::psql::TABLE_KEYWORDS;

/// File extensions whose SQL is extracted from tagged template literals
pub const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];

/// Component file extensions whose `<script>` blocks are checked like
/// [`EXTENSIONS`] files
pub const COMPONENT_EXTENSIONS: &[&str] = &["vue", "svelte"];

/// Directories skipped when a glob pattern matches TypeScript or JavaScript files
/// in them: installed packages and build output
pub const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "out",
];

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
    has_extension(path, EXTENSIONS) || is_component_file(path)
}

/// Whether `path` is a Vue or Svelte component (see [`COMPONENT_EXTENSIONS`]),
/// whose `<script>` blocks are checked
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::embedded::is_component_file;
///
/// assert!(is_component_file(Path::new("src/App.vue")));
/// assert!(!is_component_file(Path::new("src/db.ts")));
/// ```
pub fn is_component_file(path: &Path) -> bool {
    has_extension(path, COMPONENT_EXTENSIONS)
}

/// Whether `path` is in a directory of installed packages or build output (see
/// [`SKIPPED_DIRECTORIES`])
///
/// # Example
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::embedded::is_in_skipped_directory;
///
/// assert!(is_in_skipped_directory(Path::new("node_modules/pg/index.js")));
/// assert!(is_in_skipped_directory(Path::new("web/dist/app.js")));
/// assert!(!is_in_skipped_directory(Path::new("src/db/posts.ts")));
/// ```
pub fn is_in_skipped_directory(path: &Path) -> bool {
    path.parent().is_some_and(|dir| {
        dir.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name))
        })
    })
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The kind of file SQL is extracted from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Host {
    /// A TypeScript or JavaScript file
    Script,
    /// A Vue or Svelte component: only its `<script>` blocks are code
    Component,
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

/// Keywords a statement starts with: a template that doesn't start with one is a
/// query fragment
const STATEMENT_KEYWORDS: &[&str] = &[
    "select",
    "with",
    "insert",
    "update",
    "delete",
    "values",
    "table",
    "create",
    "alter",
    "drop",
    "truncate",
    "merge",
    "replace",
    "upsert",
    "explain",
    "analyze",
    "vacuum",
    "show",
    "set",
    "reset",
    "begin",
    "start",
    "commit",
    "rollback",
    "savepoint",
    "release",
    "grant",
    "revoke",
    "copy",
    "call",
    "do",
    "lock",
    "declare",
    "prepare",
    "execute",
    "deallocate",
    "listen",
    "notify",
    "unlisten",
    "comment",
    "refresh",
    "reindex",
    "cluster",
    "discard",
    "use",
    "describe",
    "desc",
    "pragma",
    "attach",
    "detach",
];

/// Keywords after which an interpolation is a value (a placeholder), not a
/// fragment between clauses
const VALUE_KEYWORDS: &[&str] = &[
    "select",
    "where",
    "and",
    "or",
    "not",
    "on",
    "by",
    "limit",
    "offset",
    "like",
    "ilike",
    "in",
    "is",
    "then",
    "else",
    "when",
    "case",
    "between",
    "distinct",
    "having",
    "returning",
    "interval",
    "any",
    "all",
    "some",
    "exists",
    "fetch",
    "first",
    "next",
    "top",
    "escape",
    "similar",
    "to",
    "zone",
    "regexp",
    "rlike",
    "glob",
    "match",
    "array",
    "row",
    "against",
];

/// A `sqlsift:` directive in a code comment, written to the SQL as a `--` comment
struct Directive {
    /// Index of the comment's first character
    start: usize,
    /// The directive's text (from `sqlsift:`), on the comment's first line
    text: Vec<char>,
    /// Whether whitespace separates the directive from `//` or `/*`
    space: bool,
    /// Index after the comment, when it ends on its first line
    end: Option<usize>,
}

/// Extract the SQL of the templates tagged with one of `tags` (see the module docs)
pub(crate) fn extract<S: AsRef<str>>(
    source: &str,
    tags: &[S],
    dialect: SqlDialect,
    host: Host,
) -> Extracted {
    let mut chars: Vec<char> = source.chars().collect();
    if host == Host::Component {
        mask_markup(&mut chars);
    }
    let n = chars.len();
    let mut out: Vec<char> = chars
        .iter()
        .map(|&c| if matches!(c, '\n' | '\r') { c } else { ' ' })
        .collect();
    // Substituted identifiers as character index ranges
    let mut identifiers = Vec::new();
    let mut directives = Vec::new();

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
                let start = i;
                while i < n && chars[i] != '\n' {
                    i += 1;
                }
                directives.extend(directive(&chars[start + 2..i], start, Some(i)));
            }
            '/' if next == Some('*') => {
                let start = i;
                i += 2;
                while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                let first_line_end = (start..i).find(|&k| chars[k] == '\n');
                let text_end = first_line_end.unwrap_or(i);
                i = (i + 2).min(n);
                let end = first_line_end.is_none().then_some(i);
                directives.extend(directive(&chars[start + 2..text_end], start, end));
            }
            '\'' | '"' => {
                i = skip_string(&chars, i);
                previous = Previous::Value;
            }
            '`' => {
                let in_sql = stack
                    .iter()
                    .any(|f| matches!(f, Frame::Template { sql: true }));
                let sql =
                    !in_sql && is_tagged(&chars, i, tags) && starts_with_statement(&chars, i + 1);
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

    // Directives in code comments become `--` comments, unless SQL follows the
    // comment on its line (which the `--` would hide)
    for d in directives {
        let line_end = (d.start..n)
            .find(|&k| matches!(chars[k], '\n' | '\r'))
            .unwrap_or(n);
        if d.end
            .is_some_and(|end| out[end..line_end].iter().any(|c| !c.is_whitespace()))
        {
            continue;
        }
        // `//` or `/*` becomes `--`, the directive keeps its place or moves into
        // the whitespace before it
        out[d.start] = '-';
        out[d.start + 1] = '-';
        let text_start = d.start + 2 + usize::from(d.space);
        for (k, &c) in d.text.iter().enumerate() {
            out[text_start + k] = c;
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

/// The `sqlsift:` directive in the text `text` (after `//` or `/*`, on the first
/// line) of the comment at `start`
fn directive(text: &[char], start: usize, end: Option<usize>) -> Option<Directive> {
    let leading = text
        .iter()
        .take_while(|c| c.is_whitespace() || **c == '*' || **c == '/')
        .count();
    let prefix: Vec<char> = "sqlsift:".chars().collect();
    if !text[leading..].starts_with(&prefix) {
        return None;
    }
    let mut directive: Vec<char> = text[leading..].to_vec();
    while directive.last().is_some_and(|c| c.is_whitespace()) {
        directive.pop();
    }
    Some(Directive {
        start,
        text: directive,
        space: leading > 0,
        end,
    })
}

/// Replace everything outside the `<script>` blocks of a component with spaces
/// (keeping line breaks)
fn mask_markup(chars: &mut [char]) {
    let lower: Vec<char> = chars.iter().map(char::to_ascii_lowercase).collect();
    let find = |from: usize, pattern: &str| -> Option<usize> {
        let pattern: Vec<char> = pattern.chars().collect();
        (from..lower.len()).find(|&k| lower[k..].starts_with(&pattern))
    };
    let mut code = vec![false; chars.len()];
    let mut i = 0;
    while let Some(open) = find(i, "<script") {
        let after = open + "<script".len();
        if !lower
            .get(after)
            .is_some_and(|c| c.is_whitespace() || *c == '>')
        {
            i = after;
            continue;
        }
        let Some(tag_end) = find(after, ">") else {
            break;
        };
        let close = find(tag_end + 1, "</script").unwrap_or(chars.len());
        for flag in &mut code[tag_end + 1..close] {
            *flag = true;
        }
        i = close + 1;
    }
    for (c, code) in chars.iter_mut().zip(code) {
        if !code && !matches!(*c, '\n' | '\r') {
            *c = ' ';
        }
    }
}

/// Whether the template text starting at `i` starts with a statement keyword
/// (after whitespace, SQL comments and opening parentheses)
fn starts_with_statement(chars: &[char], mut i: usize) -> bool {
    let n = chars.len();
    loop {
        match (chars.get(i), chars.get(i + 1)) {
            (Some(c), _) if c.is_whitespace() || *c == '(' => i += 1,
            (Some('-'), Some('-')) => {
                while i < n && !matches!(chars[i], '\n' | '`') {
                    i += 1;
                }
            }
            (Some('/'), Some('*')) => {
                i += 2;
                while i < n
                    && chars[i] != '`'
                    && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/'))
                {
                    i += 1;
                }
                i += 2;
            }
            _ => break,
        }
    }
    let start = i.min(n);
    let mut end = start;
    while end < n && chars[end].is_ascii_alphabetic() {
        end += 1;
    }
    let word = chars[start..end]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    STATEMENT_KEYWORDS.contains(&word.as_str())
        // A misspelled statement (`SELEC id FROM t`) is reported as a syntax error
        || ["select", "insert", "update", "delete"]
            .iter()
            .any(|k| is_misspelling(&word, k))
}

/// Whether `word` is `keyword` with one character changed, removed or swapped
/// with the next one (not added: `deleted` is a column name)
fn is_misspelling(word: &str, keyword: &str) -> bool {
    let (w, k): (Vec<char>, Vec<char>) = (word.chars().collect(), keyword.chars().collect());
    let prefix = w.iter().zip(&k).take_while(|(a, b)| a == b).count();
    if w.len() == k.len() {
        // One substitution or one transposition
        prefix < w.len()
            && (w[prefix + 1..] == k[prefix + 1..]
                || (prefix + 1 < w.len()
                    && w[prefix] == k[prefix + 1]
                    && w[prefix + 1] == k[prefix]
                    && w[prefix + 2..] == k[prefix + 2..]))
    } else {
        w.len() + 1 == k.len() && w[prefix..] == k[prefix + 1..]
    }
}

/// Whether the tag of the template whose backtick is at `i` is one of `tags`: the
/// last or the first identifier of the tag expression, a chain of member accesses
/// and calls (`db.sql`, `prisma.$queryRaw<User[]>`, `sql.type(schema)`)
fn is_tagged<S: AsRef<str>>(chars: &[char], i: usize, tags: &[S]) -> bool {
    let names = tag_names(chars, i);
    let is_tag = |name: &String| tags.iter().any(|t| t.as_ref() == name);
    names.first().is_some_and(is_tag) || names.last().is_some_and(is_tag)
}

/// The identifiers of the tag expression of the template whose backtick is at `i`,
/// last first
fn tag_names(chars: &[char], i: usize) -> Vec<String> {
    let skip_space = |mut j: usize| {
        while j > 0 && chars[j - 1].is_whitespace() {
            j -= 1;
        }
        j
    };
    let mut names = Vec::new();
    let mut end = skip_space(i);
    if end > 0 && chars[end - 1] == '>' {
        // Type arguments, on the same line
        let Some(j) = skip_back(chars, end, '<', '>', true) else {
            return names;
        };
        end = skip_space(j);
    }
    loop {
        // Calls: `sql.type(schema)`
        while end > 0 && chars[end - 1] == ')' {
            let Some(j) = skip_back(chars, end, '(', ')', false) else {
                return names;
            };
            end = skip_space(j);
        }
        let mut start = end;
        while start > 0 && is_ident_char(chars[start - 1]) {
            start -= 1;
        }
        if start == end {
            return names;
        }
        names.push(chars[start..end].iter().collect());
        // A member access: `.name` or `?.name`
        let dot = skip_space(start);
        if dot == 0 || chars[dot - 1] != '.' {
            return names;
        }
        end = dot - 1;
        if end > 0 && chars[end - 1] == '?' {
            end -= 1;
        }
        end = skip_space(end);
    }
}

/// The index of the `open` bracket matching the `close` bracket at `end - 1`, or
/// `None` if there is none nearby (on the same line, if `same_line`)
fn skip_back(
    chars: &[char],
    end: usize,
    open: char,
    close: char,
    same_line: bool,
) -> Option<usize> {
    const LIMIT: usize = 2000;
    let mut depth = 0;
    let mut j = end;
    loop {
        if j == 0 || end - j > LIMIT || matches!(chars[j - 1], ';' | '`') {
            return None;
        }
        if same_line && chars[j - 1] == '\n' {
            return None;
        }
        j -= 1;
        if chars[j] == close {
            depth += 1;
        } else if chars[j] == open {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
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
    let parameter = match dialect {
        SqlDialect::PostgreSQL => "$1",
        SqlDialect::MySQL | SqlDialect::SQLite => "?",
    };

    // The SQL word before the interpolation
    let word_end = skip_space_back(out, start);
    let mut word_start = word_end;
    while word_start > 0 && is_sql_word_char(out[word_start - 1]) {
        word_start -= 1;
    }
    let word: String = out[word_start..word_end].iter().collect();
    let is_word = |k: &&str| word.eq_ignore_ascii_case(k);

    let first_line_end = (start..end).find(|&k| chars[k] == '\n').unwrap_or(end);
    let width = first_line_end - start;
    if TABLE_KEYWORDS.iter().any(is_word) {
        // An identifier as long as the interpolation's first line: `__table_`
        write_identifier(chars, out, start, first_line_end);
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

    // postgres.js helpers: columns and values the query doesn't show
    if word.eq_ignore_ascii_case("set") && width > parameter.len() + 1 {
        // `SET ${sql(patch)}`: `SET <identifier>=$1`
        let name_end = first_line_end - parameter.len() - 1;
        write_identifier(chars, out, start, name_end);
        write(out, name_end, &format!("={parameter}"));
        return Some((start, name_end));
    }
    // Rows of unknown columns are `TABLE <schema>.<name>` (sqlparser 0.53 reads a
    // `TABLE` query as three tokens, so the name is qualified)
    if word.eq_ignore_ascii_case("values") && width >= "_.x".len() {
        // `VALUES ${sql(rows)}`: `TABLE  _.<identifier>`
        write(out, word_start, "TABLE ");
        write_qualified_identifier(chars, out, start, first_line_end);
        return Some((start, first_line_end));
    }
    if (word_start == word_end || !VALUE_KEYWORDS.iter().any(is_word))
        && width >= "TABLE _.x".len()
        && follows_insert_into(out, word_end)
    {
        // `INSERT INTO users ${sql(user)}`: `INSERT INTO users TABLE _.<identifier>`
        write(out, start, "TABLE ");
        let name_start = start + "TABLE ".len();
        write_qualified_identifier(chars, out, name_start, first_line_end);
        return Some((name_start, first_line_end));
    }

    // After a value or a name, an interpolation is a fragment between clauses
    // (`WHERE a = 1 ${filter}`, `FROM users ${where}`): dropped
    let after_value = if word_start < word_end {
        !VALUE_KEYWORDS.iter().any(is_word)
    } else {
        word_end > 0
            && (matches!(out[word_end - 1], ')' | ']' | '\'' | '"' | '`')
                || (out[word_end - 1] == '?' && dialect != SqlDialect::PostgreSQL))
    };
    if after_value {
        return None;
    }

    // A list after IN (`IN ${ids}`) needs parentheses
    let text = if word.eq_ignore_ascii_case("in") && width >= parameter.len() + 2 {
        format!("({parameter})")
    } else {
        parameter.to_string()
    };
    write(out, start, &text);
    None
}

fn is_sql_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The index after the last non-whitespace character of `out[..end]`
fn skip_space_back(out: &[char], mut end: usize) -> usize {
    while end > 0 && out[end - 1].is_whitespace() {
        end -= 1;
    }
    end
}

/// Whether the SQL text `out[..end]` ends with `INTO <table name>`
fn follows_insert_into(out: &[char], end: usize) -> bool {
    let mut name_start = end;
    while name_start > 0
        && (is_sql_word_char(out[name_start - 1]) || matches!(out[name_start - 1], '.' | '"' | '`'))
    {
        name_start -= 1;
    }
    if name_start == end {
        return false;
    }
    let into_end = skip_space_back(out, name_start);
    let into_start = into_end.saturating_sub("into".len());
    let into: String = out[into_start..into_end].iter().collect();
    into.eq_ignore_ascii_case("into") && (into_start == 0 || !is_sql_word_char(out[into_start - 1]))
}

fn write(out: &mut [char], start: usize, text: &str) {
    for (k, c) in text.chars().enumerate() {
        out[start + k] = c;
    }
}

/// Write a qualified name (`_.<identifier>`) made of the characters of
/// `chars[start..end]` to `out`
fn write_qualified_identifier(chars: &[char], out: &mut [char], start: usize, end: usize) {
    write(out, start, "_.");
    write_identifier(chars, out, start + 2, end);
}

/// Write an identifier made of the characters of `chars[start..end]` to `out`
fn write_identifier(chars: &[char], out: &mut [char], start: usize, end: usize) {
    for k in start..end {
        let c = chars[k];
        out[k] = if k > start && is_sql_word_char(c) {
            c
        } else {
            '_'
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql(source: &str) -> String {
        let text = extract(source, DEFAULT_TAGS, SqlDialect::PostgreSQL, Host::Script).text;
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
        let extract = |s: &str| extract(s, &tags, SqlDialect::PostgreSQL, Host::Script).text;
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
        let mysql = extract(
            "sql`DELETE FROM t WHERE a = ${x}`",
            DEFAULT_TAGS,
            SqlDialect::MySQL,
            Host::Script,
        )
        .text;
        assert_eq!(mysql, "   ;DELETE FROM t WHERE a = ?   ;");
    }

    #[test]
    fn interpolated_table_names_become_identifiers() {
        let extracted = extract(
            "sql`SELECT 1\nFROM ${table} JOIN ${s}.users`",
            DEFAULT_TAGS,
            SqlDialect::PostgreSQL,
            Host::Script,
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
            sql("sql`SELECT 1 WHERE ${sql`x`}`"),
            "   ;SELECT 1 WHERE $1       ;"
        );
    }

    #[test]
    fn interpolations_between_clauses_are_dropped() {
        assert_eq!(
            sql("sql`SELECT 1 FROM t WHERE a = ${a} ${b ? sql`AND b` : sql``} LIMIT ${n}`"),
            "   ;SELECT 1 FROM t WHERE a = $1                             LIMIT $1  ;"
        );
        assert_eq!(
            sql("sql`SELECT 1 FROM t ${where}`"),
            "   ;SELECT 1 FROM t         ;"
        );
        assert_eq!(
            sql("sql`SELECT f(a) ${x} FROM t AS \"q\" ${y}`"),
            "   ;SELECT f(a)      FROM t AS \"q\"     ;"
        );
    }

    #[test]
    fn postgres_js_helpers_are_unknown_rows_and_columns() {
        let extracted = extract(
            "sql`INSERT INTO users ${sql(user, 'a')}`;\n\
             sql`INSERT INTO t (a) VALUES ${sql(rows)}`;\n\
             sql`UPDATE t SET ${sql(patch)} WHERE id = ${id}`;\n\
             sql`INSERT INTO \"t\" ${sql(r)}`;",
            DEFAULT_TAGS,
            SqlDialect::PostgreSQL,
            Host::Script,
        );
        assert_eq!(
            extracted.text,
            "   ;INSERT INTO users TABLE _._r___a___; \n\
             \x20  ;INSERT INTO t (a) TABLE  _._ql_rows__; \n\
             \x20  ;UPDATE t SET __sql_patc=$1 WHERE id = $1   ; \n\
             \x20  ;INSERT INTO \"t\" TABLE _._; "
        );
        assert_eq!(
            extracted.identifiers,
            vec![(1, 29, 40), (2, 30, 42), (3, 18, 28), (4, 27, 30)]
        );
    }

    #[test]
    fn fragments_are_not_extracted() {
        for source in [
            "sql`published = ${x}`",
            "sql<boolean>`a = ${x}`",
            "Prisma.sql`WHERE id > ${id}`",
            "sql`AND x`",
            "sql``",
            "sql`${a} ${b}`",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
        for source in [
            "sql`SELEC 1`",
            "sql`slect 1`",
            "sql`SELETC 1`",
            "sql`UPDTE t`",
        ] {
            assert_ne!(sql(source).trim(), "", "{source}");
        }
        for source in ["sql`deleted = true`", "sql`selected`", "sql`sel`"] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
        assert_eq!(
            sql("sql` -- c\n /* d */ (SELECT 1)`").trim(),
            "; -- c\n /* d */ (SELECT 1);"
        );
    }

    #[test]
    fn tags_may_be_calls_and_members_of_the_tag() {
        for source in [
            "sql.type(z.object({ id: z.number() }))`SELECT 1`",
            "sql.typeAlias('id')`SELECT 1`",
            "sql.unsafe`SELECT 1`",
            "sql\n  .type(row)`SELECT 1`",
            "db?.sql`SELECT 1`",
        ] {
            assert_eq!(sql(source).trim(), ";SELECT 1;", "{source}");
        }
        for source in [
            "other.type(sql)`SELECT 1`",
            "a.sql.b`SELECT 1`",
            "f(sql)`SELECT 1`",
        ] {
            assert_eq!(sql(source).trim(), "", "{source}");
        }
    }

    #[test]
    fn directives_in_code_comments_become_sql_comments() {
        assert_eq!(
            sql(
                "// sqlsift:disable-file\n/* sqlsift:disable E0002 */\n//sqlsift:disable\n// other"
            ),
            "-- sqlsift:disable-file\n-- sqlsift:disable E0002   \n--sqlsift:disable\n        "
        );
        // Not when the `--` would hide SQL after the comment
        assert_eq!(
            sql("/* sqlsift:disable */ sql`SELECT 1`").trim(),
            ";SELECT 1;"
        );
        assert_eq!(
            sql("sql`SELECT 1`; // sqlsift:disable E0002"),
            "   ;SELECT 1;  -- sqlsift:disable E0002"
        );
    }

    #[test]
    fn components_check_their_script_blocks() {
        let source = "<template>\n  <p>{{ sql`SELECT 2` }}</p>\n</template>\n\
                      <script setup lang=\"ts\">\nconst q = sql`SELECT 1`;\n</script>\n\
                      <style>a { b: c }</style>";
        let text = extract(
            source,
            DEFAULT_TAGS,
            SqlDialect::PostgreSQL,
            Host::Component,
        )
        .text;
        assert_eq!(text.lines().count(), source.lines().count());
        assert_eq!(text.trim(), ";SELECT 1;");
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
        for name in [
            "a.ts", "a.tsx", "a.js", "a.jsx", "a.mts", "a.cts", "a.mjs", "A.TS", "a.vue",
            "a.svelte",
        ] {
            assert!(is_embedded_sql_file(Path::new(name)), "{name}");
        }
        for name in ["a.sql", "a", "ts"] {
            assert!(!is_embedded_sql_file(Path::new(name)), "{name}");
        }
    }
}
