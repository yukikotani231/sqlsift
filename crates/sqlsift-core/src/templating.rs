//! Template support for query files (dbt / Jinja)
//!
//! Query files of a dbt project are Jinja templates. With [`Templating::Jinja`],
//! `mask_jinja` rewrites the template syntax into SQL before parsing:
//!
//! - `{# comment #}` and `{% statement %}` become whitespace. The SQL inside a
//!   `{% for %}` block is checked once, as its first and last iteration (so
//!   `{% if not loop.last %},{% endif %}` and `{{ ',' if not loop.last }}` are
//!   dropped). Of an `{% if %}` / `{% elif %}` / `{% else %}` block only the first
//!   branch is kept; the others are blanked.
//! - The bodies of `{% set x %}...{% endset %}`, `{% call %}`, `{% macro %}`,
//!   `{% test %}`, `{% materialization %}` and `{% docs %}` blocks are blanked;
//!   the body of `{% raw %}...{% endraw %}` is SQL text.
//! - `{{ source('schema', 'table') }}` where a table name is expected becomes
//!   `schema.table` when the schema has that table.
//! - Any other `{{ expression }}` where a table name is expected (after FROM, JOIN,
//!   INTO, UPDATE, TABLE or USING, e.g. `FROM {{ ref('orders') }}`) becomes an
//!   identifier made from its text (`___ref__orders____`). It names a table sqlsift
//!   doesn't know, so its columns are unknown: name diagnostics reported on it are
//!   dropped with `Preprocessed::is_substituted`, and columns that may come from it
//!   are not reported. The same goes for an alias (`AS {{ name }}`), a type
//!   (`x::{{ dbt.type_bigint() }}`) and an expression that is part of a name
//!   (`{{ prefix }}_total`, `t.{{ column }}`).
//! - `{{ expression }}` at the start of a statement (`{{ config(...) }}`) becomes
//!   whitespace.
//! - A string literal with an `{{ expression }}` in it (`'{{ var("x") }}'`) and any
//!   other `{{ expression }}` become the placeholder `$1`, an untyped value like a
//!   bind parameter.
//! - `sqlsift:disable` directives in `{# ... #}` comments work like `--` comments.
//!
//! Every character is replaced by one character and line breaks are kept, so
//! parser and diagnostic line/column locations still point at the original text.

use std::borrow::Cow;
use std::str::FromStr;

use crate::psql::Preprocessed;

/// How query files are templated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Templating {
    /// Plain SQL
    #[default]
    None,
    /// Jinja templates (dbt models): template tags are masked before analysis
    Jinja,
}

impl FromStr for Templating {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" | "off" => Ok(Templating::None),
            "jinja" | "dbt" => Ok(Templating::Jinja),
            _ => Err(format!(
                "Unknown templating: '{s}'. Supported values: jinja, none."
            )),
        }
    }
}

impl std::fmt::Display for Templating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Templating::None => write!(f, "none"),
            Templating::Jinja => write!(f, "jinja"),
        }
    }
}

/// Whether `sql` looks like a Jinja template (has `{{` or `{%`)
pub(crate) fn looks_like_jinja(sql: &str) -> bool {
    sql.contains("{{") || sql.contains("{%")
}

/// Keywords after which `{{ ... }}` names a table
const TABLE_KEYWORDS: &[&str] = &["from", "join", "into", "update", "table", "using"];

/// Blocks whose body is not SQL and is blanked (`{% macro %}...{% endmacro %}`). A
/// `{% set %}` block (without `=`) also is.
const NON_SQL_BLOCKS: &[&str] = &["macro", "call", "test", "materialization", "docs", "set"];

/// Keywords before a string literal whose text is part of a value
/// (`interval '{{ n }} days'`): such a string is kept a string
const TYPED_STRING_KEYWORDS: &[&str] = &["interval", "date", "time", "timestamp"];

/// The kind of a Jinja tag, from its opening delimiter
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    /// `{# ... #}`
    Comment,
    /// `{% ... %}`
    Statement,
    /// `{{ ... }}`
    Expression,
}

/// Byte index just past the tag that starts at `start` (the end of the input if
/// it isn't closed). Quoted strings inside statements and expressions may contain
/// the closing delimiter.
fn tag_end(sql: &str, start: usize, tag: Tag) -> usize {
    let bytes = sql.as_bytes();
    let close: &[u8] = match tag {
        Tag::Comment => b"#}",
        Tag::Statement => b"%}",
        Tag::Expression => b"}}",
    };
    let mut i = start + 2;
    while i < bytes.len() {
        let b = bytes[i];
        if bytes[i..].starts_with(close) {
            return i + 2;
        }
        if tag != Tag::Comment && matches!(b, b'\'' | b'"') {
            i += 1;
            while i < bytes.len() && bytes[i] != b {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
        }
        i += 1;
    }
    bytes.len()
}

/// The text of a tag between its delimiters, without whitespace control (`-`, `+`)
fn tag_content(text: &str) -> &str {
    let inner = text.get(2..).unwrap_or("");
    let closed = text.len() >= 4 && ["}}", "%}", "#}"].iter().any(|c| text.ends_with(c));
    let inner = if closed {
        &inner[..inner.len() - 2]
    } else {
        inner
    };
    inner
        .trim()
        .trim_start_matches(['-', '+'])
        .trim_end_matches(['-', '+'])
        .trim()
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where an expression tag is, judged from the (already masked) text before it and
/// the text after it
enum Position {
    /// At the start of a statement: the tag expands to something other than a value
    Statement,
    /// Where a table name is expected (`FROM {{ ref('a') }}`)
    Table,
    /// Where another name is expected: an alias (`AS {{ name }}`), a type
    /// (`x::{{ dbt.type_bigint() }}`), or part of a name (`{{ prefix }}_total`,
    /// `t.{{ col }}`, `{{ target.schema }}.users`)
    Name,
    /// Anywhere else: a value
    Value,
}

fn is_name_char(c: char) -> bool {
    is_ident_char(c) || matches!(c, '.' | '"')
}

/// The identifier word at the end of `text`
fn last_word(text: &str) -> &str {
    let start = text.rfind(|c: char| !is_ident_char(c)).map_or(0, |i| i + 1);
    &text[start..]
}

fn position(before: &str, after: &str) -> Position {
    let glued = before.ends_with(is_name_char) || after.starts_with(is_name_char);
    let before = before.trim_end();
    if before.is_empty() || before.ends_with(';') {
        return Position::Statement;
    }
    let word = last_word(before);
    if glued {
        Position::Name
    } else if TABLE_KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
        Position::Table
    } else if word.eq_ignore_ascii_case("as") || before.ends_with("::") {
        Position::Name
    } else {
        Position::Value
    }
}

/// A quoted name argument (`'customers'`), without its quotes
fn unquote_name(arg: &str) -> Option<&str> {
    let arg = arg.trim();
    let name = arg
        .strip_prefix('\'')
        .and_then(|a| a.strip_suffix('\''))
        .or_else(|| arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')))?;
    let valid = name.chars().all(is_ident_char)
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    valid.then_some(name)
}

/// `source('schema', 'table')`: the schema and table names
fn source_call(content: &str) -> Option<(&str, &str)> {
    let args = content
        .strip_prefix("source")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let (schema, table) = args.split_once(',')?;
    Some((unquote_name(schema)?, unquote_name(table)?))
}

/// A loop separator like `{{ ',' if not loop.last }}`
fn is_loop_separator(content: &str) -> bool {
    let Some(quote) = content.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
        return false;
    };
    let Some((literal, rest)) = content[1..].split_once(quote) else {
        return false;
    };
    literal.chars().all(|c| c == ',' || c.is_whitespace())
        && rest.trim_start().starts_with("if ")
        && rest.contains("loop.")
}

/// The value of a branch condition when it is known: the body of a `{% for %}` loop
/// is checked once, as its first and last iteration
fn branch_condition(condition: &str) -> Option<bool> {
    let words: Vec<&str> = condition.split_whitespace().collect();
    match words.as_slice() {
        ["loop.last" | "loop.first"] => Some(true),
        ["not", "loop.last" | "loop.first"] => Some(false),
        _ => None,
    }
}

/// An open `{% ... %}` block
enum Frame {
    /// `{% if %}`: whether the current branch is kept, and whether any branch was
    If { active: bool, taken: bool },
    /// `{% for %}`: whether in its `{% else %}` branch (blanked)
    For { in_else: bool },
    /// A block whose body is blanked, ended by `{% end<keyword> %}`
    Blank(String),
}

/// Where the text being copied is, lexically
#[derive(Clone, Copy)]
enum Lex {
    Code,
    /// In a `'...'` string starting at this byte of the output, which contains a
    /// template expression when `templated`
    String {
        start: usize,
        templated: bool,
    },
    QuotedIdentifier,
    LineComment,
    BlockComment,
}

/// A template tag that became a value or a name, for parse error messages
struct TagSpan {
    line: usize,
    start: usize,
    end: usize,
    text: String,
}

/// A query file with its Jinja template syntax masked (see the module docs)
pub(crate) struct Template<'a> {
    /// The masked text and its substituted identifiers
    pub masked: Preprocessed<'a>,
    /// `sqlsift:` directives in `{# ... #}` comments: (line, directive)
    directives: Vec<(usize, String)>,
    /// Tags that became values or names
    tags: Vec<TagSpan>,
}

impl<'a> Template<'a> {
    /// A file without templating
    pub fn unchanged(sql: &'a str) -> Self {
        Template {
            masked: Preprocessed::unchanged(sql),
            directives: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// The text to read `sqlsift:disable` directives from: the masked text (so lines
    /// holding only template tags are blank), with the directives of `{# ... #}`
    /// comments as `--` comments
    pub fn directive_text(&self) -> Cow<'_, str> {
        if self.directives.is_empty() {
            return Cow::Borrowed(&self.masked.text);
        }
        let lines: Vec<String> = self
            .masked
            .text
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let mut line = line.trim_end_matches('\r').to_string();
                for (_, directive) in self.directives.iter().filter(|(l, _)| *l == i + 1) {
                    line.push_str(" -- ");
                    line.push_str(directive);
                }
                line
            })
            .collect();
        Cow::Owned(lines.join("\n"))
    }

    /// The original text (first line) of the template tag at `line`/`column`, if one
    /// became a value or a name there
    pub fn tag_at(&self, line: usize, column: usize) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.line == line && (t.start..t.end).contains(&column))
            .map(|t| t.text.as_str())
    }
}

/// State of [`mask_jinja`]
struct Masker<'s, 'f> {
    sql: &'s str,
    table_exists: &'f dyn Fn(&str, &str) -> bool,
    out: String,
    identifiers: Vec<(usize, usize, usize)>,
    directives: Vec<(usize, String)>,
    tags: Vec<TagSpan>,
    frames: Vec<Frame>,
    lex: Lex,
    /// The previous character was the first quote of a doubled quote (`''`) in a
    /// string
    quote_pending: bool,
    /// Line and character column of the next character
    line: usize,
    column: usize,
}

impl Masker<'_, '_> {
    /// Whether the text at this point is part of the SQL: outside blanked blocks and
    /// branches
    fn active(&self) -> bool {
        self.frames.iter().all(|frame| match frame {
            Frame::If { active, .. } => *active,
            Frame::For { in_else } => !in_else,
            Frame::Blank(_) => false,
        })
    }

    /// Move the line/column past `text`
    fn advance(&mut self, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    /// Copy the SQL text `sql[range]` (blanked when not [`Self::active`])
    fn copy(&mut self, range: std::ops::Range<usize>) {
        let sql = self.sql;
        if !self.active() {
            blank(&mut self.out, &sql[range.clone()]);
            self.advance(&sql[range]);
            return;
        }
        let mut prev = '\0';
        for (i, c) in sql[range.clone()].char_indices() {
            let at = range.start + i;
            let next = sql[at + c.len_utf8()..].chars().next();
            let mut closed_string = None;
            self.lex = match self.lex {
                Lex::Code => match c {
                    '\'' => Lex::String {
                        start: self.out.len(),
                        templated: false,
                    },
                    '"' => Lex::QuotedIdentifier,
                    '-' if next == Some('-') => Lex::LineComment,
                    '/' if next == Some('*') => Lex::BlockComment,
                    _ => Lex::Code,
                },
                Lex::String { start, templated } if c == '\'' => {
                    if self.quote_pending {
                        // The second quote of `''`
                        self.quote_pending = false;
                        Lex::String { start, templated }
                    } else if next == Some('\'') {
                        self.quote_pending = true;
                        Lex::String { start, templated }
                    } else {
                        closed_string = templated.then_some(start);
                        Lex::Code
                    }
                }
                Lex::QuotedIdentifier if c == '"' => Lex::Code,
                Lex::LineComment if c == '\n' => Lex::Code,
                Lex::BlockComment if c == '/' && prev == '*' => Lex::Code,
                lex => lex,
            };
            self.out.push(c);
            self.advance(&sql[at..at + c.len_utf8()]);
            if let Some(start) = closed_string {
                self.untype_string(start);
            }
            prev = c;
        }
    }

    /// A string literal (from byte `start` of the output to its end) with a template
    /// expression in it renders to any literal (`'{{ var("x") }}'`): make it the
    /// untyped placeholder `$1`, unless it is part of a typed literal
    /// (`interval '{{ n }} days'`)
    fn untype_string(&mut self, start: usize) {
        let keyword = last_word(self.out[..start].trim_end());
        if TYPED_STRING_KEYWORDS
            .iter()
            .any(|k| keyword.eq_ignore_ascii_case(k))
        {
            return;
        }
        let literal = self.out.split_off(start);
        self.out.push_str("$1");
        blank(&mut self.out, &literal[2..]);
    }

    /// Handle a `{% ... %}` tag (blanked), updating the open blocks. Returns the end
    /// of the text it covers (past `{% endraw %}` for `{% raw %}`).
    fn statement(&mut self, start: usize, end: usize) -> usize {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        let keyword_end = content
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(content.len());
        let (keyword, args) = content.split_at(keyword_end);
        let keyword = keyword.to_ascii_lowercase();
        blank(&mut self.out, text);
        self.advance(text);
        match keyword.as_str() {
            "if" => {
                let active = branch_condition(args) != Some(false);
                self.frames.push(Frame::If {
                    active,
                    taken: active,
                });
            }
            "elif" => {
                if let Some(Frame::If { active, taken }) = self.frames.last_mut() {
                    *active = !*taken && branch_condition(args) != Some(false);
                    *taken |= *active;
                }
            }
            "else" => match self.frames.last_mut() {
                Some(Frame::If { active, taken }) => {
                    *active = !*taken;
                    *taken = true;
                }
                Some(Frame::For { in_else }) => *in_else = true,
                _ => {}
            },
            "for" => self.frames.push(Frame::For { in_else: false }),
            "endif" | "endfor" => {
                let top_matches = match self.frames.last() {
                    Some(Frame::If { .. }) => keyword == "endif",
                    Some(Frame::For { .. }) => keyword == "endfor",
                    _ => false,
                };
                if top_matches {
                    self.frames.pop();
                }
            }
            "raw" => {
                // The body is SQL text, with no tags in it
                let (body_end, raw_end) = find_endraw(self.sql, end);
                self.copy(end..body_end);
                let end_tag = &self.sql[body_end..raw_end];
                blank(&mut self.out, end_tag);
                self.advance(end_tag);
                return raw_end;
            }
            kw if NON_SQL_BLOCKS.contains(&kw) && !(kw == "set" && args.contains('=')) => {
                self.frames.push(Frame::Blank(format!("end{kw}")));
            }
            kw => {
                if matches!(self.frames.last(), Some(Frame::Blank(end)) if end == kw) {
                    self.frames.pop();
                }
            }
        }
        end
    }

    /// Handle a `{{ ... }}` tag
    fn expression(&mut self, start: usize, end: usize) {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        let position = if !self.active() || is_loop_separator(content) {
            Position::Statement
        } else {
            match &mut self.lex {
                Lex::String { templated, .. } => {
                    // The whole string literal becomes a value
                    *templated = true;
                    Position::Statement
                }
                Lex::Code => position(&self.out, &self.sql[end..]),
                _ => Position::Statement,
            }
        };
        let first_line = text.find('\n').map_or(text, |n| &text[..n]);
        let single_line = first_line.len() == text.len();
        let column = self.column;
        let source = source_call(content)
            .filter(|&(schema, table)| single_line && (self.table_exists)(schema, table));
        match (position, source) {
            (Position::Table, Some((schema, table))) => {
                // `{{ source('raw', 'customers') }}` → `raw.customers`, a table of
                // the schema
                let name = format!("{schema}.{table}");
                let padding = text.chars().count() - name.len();
                self.out.push_str(&name);
                self.out.push_str(&" ".repeat(padding));
            }
            (Position::Table | Position::Name, _) => {
                // An identifier made from the tag's first line (`{{ ref('a') }}` →
                // `___ref__a____`), including the name it is part of
                // (`{{ target.schema }}.users`, `total_{{ c }}`)
                let glued_before = self
                    .out
                    .chars()
                    .rev()
                    .take_while(|&c| is_name_char(c))
                    .count();
                let start_column = column - glued_before;
                self.out.extend(
                    first_line
                        .chars()
                        .map(|c| if is_ident_char(c) { c } else { '_' }),
                );
                let mut end_column = column + first_line.chars().count();
                if single_line {
                    end_column += self.sql[end..]
                        .chars()
                        .take_while(|&c| is_name_char(c))
                        .count();
                }
                self.identifiers.push((self.line, start_column, end_column));
                self.tags.push(TagSpan {
                    line: self.line,
                    start: start_column,
                    end: end_column,
                    text: first_line.to_string(),
                });
                blank(&mut self.out, &text[first_line.len()..]);
            }
            (Position::Value, _) => {
                // `$1` padded with spaces (`{{` is two characters)
                self.out.push_str("$1");
                blank(&mut self.out, &text[2..]);
                self.tags.push(TagSpan {
                    line: self.line,
                    start: column,
                    end: column + first_line.chars().count(),
                    text: first_line.to_string(),
                });
            }
            (Position::Statement, _) => blank(&mut self.out, text),
        }
        self.advance(text);
    }

    /// Handle a `{# ... #}` tag (blanked); a `sqlsift:` directive in it is recorded
    fn comment(&mut self, start: usize, end: usize) {
        let text = &self.sql[start..end];
        let content = tag_content(text);
        if content.starts_with("sqlsift:") && !content.contains('\n') {
            self.directives.push((self.line, content.to_string()));
        }
        blank(&mut self.out, text);
        self.advance(text);
    }
}

/// Start and end of the `{% endraw %}` tag after byte `from` (the end of the input
/// if there is none)
fn find_endraw(sql: &str, from: usize) -> (usize, usize) {
    let mut i = from;
    while let Some(offset) = sql[i..].find("{%") {
        let start = i + offset;
        let end = tag_end(sql, start, Tag::Statement);
        if tag_content(&sql[start..end]).eq_ignore_ascii_case("endraw") {
            return (start, end);
        }
        i = start + 2;
    }
    (sql.len(), sql.len())
}

/// Mask Jinja template syntax (see the module docs). `table_exists(schema, table)`
/// tells whether the schema has a table for `{{ source(schema, table) }}`.
pub(crate) fn mask_jinja<'a>(
    sql: &'a str,
    table_exists: &dyn Fn(&str, &str) -> bool,
) -> Template<'a> {
    if !sql.contains("{{") && !sql.contains("{%") && !sql.contains("{#") {
        return Template::unchanged(sql);
    }

    let mut masker = Masker {
        sql,
        table_exists,
        out: String::with_capacity(sql.len()),
        identifiers: Vec::new(),
        directives: Vec::new(),
        tags: Vec::new(),
        frames: Vec::new(),
        lex: Lex::Code,
        quote_pending: false,
        line: 1,
        column: 1,
    };
    let mut rest = 0;

    while let Some(offset) = sql[rest..].find('{') {
        let start = rest + offset;
        let tag = match sql.as_bytes().get(start + 1) {
            Some(b'#') => Some(Tag::Comment),
            Some(b'%') => Some(Tag::Statement),
            Some(b'{') => Some(Tag::Expression),
            _ => None,
        };
        // Text before the tag (or up to and including a lone `{`) is copied as is
        let copy_end = if tag.is_some() { start } else { start + 1 };
        masker.copy(rest..copy_end);
        rest = copy_end;
        let Some(tag) = tag else {
            continue;
        };

        let end = tag_end(sql, start, tag);
        rest = match tag {
            Tag::Comment => {
                masker.comment(start, end);
                end
            }
            Tag::Statement => masker.statement(start, end),
            Tag::Expression => {
                masker.expression(start, end);
                end
            }
        };
    }
    masker.copy(rest..sql.len());

    Template {
        masked: Preprocessed::new(Cow::Owned(masker.out), masker.identifiers),
        directives: masker.directives,
        tags: masker.tags,
    }
}

/// Append `text` with every character but line breaks replaced by a space
fn blank(out: &mut String, text: &str) {
    out.extend(
        text.chars()
            .map(|c| if matches!(c, '\n' | '\r') { c } else { ' ' }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(sql: &str) -> Template<'_> {
        mask_jinja(sql, &|schema, table| {
            (schema, table) == ("raw", "customers")
        })
    }

    fn mask(sql: &str) -> String {
        let result = template(sql).masked.text.into_owned();
        assert_eq!(
            result.chars().count(),
            sql.chars().count(),
            "{sql:?} -> {result:?}"
        );
        assert_eq!(
            result.lines().count(),
            sql.lines().count(),
            "{sql:?} -> {result:?}"
        );
        result
    }

    #[test]
    fn parses_templating_values() {
        assert_eq!("jinja".parse(), Ok(Templating::Jinja));
        assert_eq!("dbt".parse(), Ok(Templating::Jinja));
        assert_eq!("None".parse(), Ok(Templating::None));
        assert!("mustache".parse::<Templating>().is_err());
        assert_eq!(Templating::Jinja.to_string(), "jinja");
    }

    #[test]
    fn blanks_comments_and_statements() {
        assert_eq!(mask("{# c #}SELECT 1"), "       SELECT 1");
        assert_eq!(
            mask("SELECT 1 {% if x %}\nWHERE a{%- endif -%}"),
            "SELECT 1           \nWHERE a             "
        );
        assert_eq!(mask("{#\nmulti\n#}\nSELECT 1"), "  \n     \n  \nSELECT 1");
    }

    #[test]
    fn expressions_become_placeholders() {
        assert_eq!(mask("SELECT {{ x }} AS a"), "SELECT $1      AS a");
        assert_eq!(
            mask("WHERE d > '{{ var(\"start\") }}'"),
            "WHERE d > $1                  "
        );
    }

    #[test]
    fn strings_with_expressions_are_values() {
        assert_eq!(mask("WHERE a = 'x{{ y }}z'"), "WHERE a = $1         ");
        // Doubled quotes, and strings without expressions, are kept
        assert_eq!(
            mask("WHERE a = 'it''s {{ y }}' OR b = 'c'"),
            "WHERE a = $1              OR b = 'c'"
        );
        // Typed literals stay strings
        assert_eq!(
            mask("WHERE d > now() - interval '{{ n }} days'"),
            "WHERE d > now() - interval '        days'"
        );
        // Quotes in comments and quoted identifiers don't start strings
        assert_eq!(
            mask("SELECT \"it's\", {{ x }} -- don't\nFROM t"),
            "SELECT \"it's\", $1      -- don't\nFROM t"
        );
    }

    #[test]
    fn expressions_in_table_position_become_identifiers() {
        assert_eq!(
            mask("SELECT * FROM {{ ref('a') }} JOIN {{ source('s','t') }} t"),
            "SELECT * FROM ___ref__a_____ JOIN ___source__s___t_____ t"
        );
    }

    #[test]
    fn known_sources_become_schema_tables() {
        assert_eq!(
            mask("SELECT * FROM {{ source('raw', 'customers') }} c"),
            "SELECT * FROM raw.customers                    c"
        );
        assert!(!template("SELECT * FROM {{ source('raw', 'customers') }}")
            .masked
            .is_substituted(1, 15));
    }

    #[test]
    fn expressions_in_names_become_identifiers() {
        assert_eq!(
            mask("SELECT 1 AS {{ c }}, total_{{ c }}, {{ c }}_x, t.{{ c }} FROM t"),
            "SELECT 1 AS ___c___, total____c___, ___c____x, t.___c___ FROM t"
        );
        assert_eq!(
            mask("SELECT id::{{ dbt.type_bigint() }} FROM t"),
            "SELECT id::___dbt_type_bigint_____ FROM t"
        );
        let p = template("SELECT total_{{ c }}_x FROM t");
        assert!(p.masked.is_substituted(1, 8));
        assert!(p.masked.is_substituted(1, 22));
        assert!(!p.masked.is_substituted(1, 24));
    }

    #[test]
    fn expression_at_statement_start_is_blanked() {
        assert_eq!(
            mask("{{ config(materialized='table') }}\nSELECT 1"),
            "                                  \nSELECT 1"
        );
        assert_eq!(mask("SELECT 1;\n{{ x }}"), "SELECT 1;\n       ");
    }

    #[test]
    fn only_the_first_branch_is_kept() {
        assert_eq!(
            mask("SELECT {% if a %}x{% elif b %}y{% else %}z{% endif %} FROM t"),
            "SELECT           x                                    FROM t"
        );
        // Nested blocks in a blanked branch stay blanked
        assert_eq!(
            mask("{% if a %}x{% else %}{% if b %}y{% endif %}{% endif %}"),
            "          x                                           "
        );
    }

    #[test]
    fn loops_are_checked_as_their_last_iteration() {
        assert_eq!(
            mask("SELECT {% for c in cs %}{{ c }}{% if not loop.last %},{% endif %}{% endfor %}"),
            "SELECT                  $1                                                   "
        );
        assert_eq!(
            mask("{% for c in cs %}x{% if loop.last %};{% else %},{% endif %}{% endfor %}"),
            "                 x                  ;                                  "
        );
        assert_eq!(
            mask("{% for c in cs %}x{{ ',' if not loop.last }}{% endfor %}"),
            "                 x                                      "
        );
        assert_eq!(
            mask("{% for c in cs %}x{% else %}y{% endfor %}"),
            "                 x                       "
        );
    }

    #[test]
    fn non_sql_blocks_are_blanked() {
        assert_eq!(
            mask("{% set s %}'a', 'b'{% endset %}SELECT 1"),
            "                               SELECT 1"
        );
        assert_eq!(
            mask("{% set s = 'a' %}SELECT 1"),
            "                 SELECT 1"
        );
        assert_eq!(
            mask("{% call statement('x') %}select 1{% endcall %}SELECT 2"),
            "                                              SELECT 2"
        );
        assert_eq!(
            mask("{% macro m(a) %}\n{{ a }} +\n{% endmacro %}"),
            "                \n         \n              "
        );
    }

    #[test]
    fn raw_bodies_are_sql_text() {
        assert_eq!(
            mask("SELECT {% raw %}'{{ x }}'{% endraw %} AS a"),
            "SELECT          '{{ x }}'             AS a"
        );
    }

    #[test]
    fn jinja_comment_directives() {
        let t = template("{# sqlsift:disable-file #}\nSELECT 1 {#- sqlsift:disable E0002 -#}");
        assert_eq!(
            t.directive_text(),
            "                           -- sqlsift:disable-file\nSELECT 1                               -- sqlsift:disable E0002"
        );
    }

    #[test]
    fn records_tag_text() {
        let t = template("SELECT {{ x }}\nFROM {{ ref('a') }}");
        assert_eq!(t.tag_at(1, 8), Some("{{ x }}"));
        assert_eq!(t.tag_at(2, 6), Some("{{ ref('a') }}"));
        assert_eq!(t.tag_at(2, 5), None);
    }

    #[test]
    fn closing_delimiters_in_strings_and_unclosed_tags() {
        assert_eq!(mask("SELECT {{ '}}' }} a"), "SELECT $1         a");
        assert_eq!(mask("SELECT 1 {% if"), "SELECT 1      ");
        assert_eq!(mask("SELECT '{' , '{x}'"), "SELECT '{' , '{x}'");
    }

    #[test]
    fn non_ascii_keeps_character_columns() {
        assert_eq!(mask("{# 日本語 #}SELECT 1"), "         SELECT 1");
    }

    #[test]
    fn records_identifier_positions() {
        let p = template("SELECT 1\nFROM {{ ref('a') }}.users x");
        assert!(p.masked.is_substituted(2, 6));
        assert!(p.masked.is_substituted(2, 25));
        assert!(!p.masked.is_substituted(2, 5));
        assert!(!p.masked.is_substituted(2, 27));
    }
}
