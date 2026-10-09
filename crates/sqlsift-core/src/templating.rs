//! Template support for query files (dbt / Jinja)
//!
//! Query files of a dbt project are Jinja templates. With [`Templating::Jinja`],
//! `mask_jinja` rewrites the template syntax into SQL before parsing:
//!
//! - `{# comment #}` and `{% statement %}` become whitespace (the SQL around and
//!   between `{% if %}` / `{% for %}` blocks is still checked).
//! - `{{ expression }}` where a table name is expected (after FROM, JOIN, INTO,
//!   UPDATE, TABLE or USING, e.g. `FROM {{ ref('orders') }}`) becomes an identifier
//!   made from its text (`___ref__orders____`). It names a table sqlsift doesn't
//!   know, so its columns are unknown: name diagnostics reported on it are dropped
//!   with `Preprocessed::is_substituted`, and columns that may come from it are not
//!   reported. The same goes for an alias (`AS {{ name }}`) and an expression
//!   that is part of a name (`{{ prefix }}_total`, `t.{{ column }}`).
//! - `{{ expression }}` at the start of a statement (`{{ config(...) }}`) becomes
//!   whitespace.
//! - Any other `{{ expression }}` becomes the placeholder `$1`, an untyped value
//!   like a bind parameter.
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

/// Keywords after which `{{ ... }}` names a table
const TABLE_KEYWORDS: &[&str] = &["from", "join", "into", "update", "table", "using"];

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

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Where an expression tag is, judged from the (already masked) text before it and
/// the text after it
enum Position {
    /// At the start of a statement: the tag expands to something other than a value
    Statement,
    /// Where a name is expected: a table name (`FROM {{ ref('a') }}`), an alias
    /// (`AS {{ name }}`), or part of a name (`{{ prefix }}_total`, `t.{{ col }}`)
    Name,
    /// Anywhere else: a value
    Value,
}

fn is_name_char(c: char) -> bool {
    is_ident_char(c) || matches!(c, '.' | '"')
}

fn position(before: &str, after: &str) -> Position {
    let glued = before.ends_with(is_name_char) || after.starts_with(is_name_char);
    let before = before.trim_end();
    if before.is_empty() || before.ends_with(';') {
        return Position::Statement;
    }
    let word_start = before
        .rfind(|c: char| !is_ident_char(c))
        .map_or(0, |i| i + 1);
    let word = &before[word_start..];
    if glued
        || word.eq_ignore_ascii_case("as")
        || TABLE_KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k))
    {
        Position::Name
    } else {
        Position::Value
    }
}

/// Mask Jinja template syntax (see the module docs)
pub(crate) fn mask_jinja(sql: &str) -> Preprocessed<'_> {
    if !sql.contains("{{") && !sql.contains("{%") && !sql.contains("{#") {
        return Preprocessed::unchanged(sql);
    }

    let mut out = String::with_capacity(sql.len());
    let mut identifiers = Vec::new();
    // Line and character column of the next character
    let mut line = 1;
    let mut column = 1;
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
        for c in sql[rest..copy_end].chars() {
            out.push(c);
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        rest = copy_end;
        let Some(tag) = tag else {
            continue;
        };

        let end = tag_end(sql, start, tag);
        let text = &sql[start..end];
        let position = match tag {
            Tag::Expression => position(&out, &sql[end..]),
            Tag::Comment | Tag::Statement => Position::Statement,
        };
        let first_line = text.find('\n').map_or(text, |n| &text[..n]);
        match position {
            Position::Name => {
                // An identifier made from the tag's first line (`{{ ref('a') }}` →
                // `___ref__a____`), including the name it is part of
                // (`{{ target.schema }}.users`, `total_{{ c }}`)
                let glued_before = out.chars().rev().take_while(|&c| is_name_char(c)).count();
                let start_column = column - glued_before;
                out.extend(
                    first_line
                        .chars()
                        .map(|c| if is_ident_char(c) { c } else { '_' }),
                );
                let mut end_column = column + first_line.chars().count();
                if first_line.len() == text.len() {
                    end_column += sql[end..].chars().take_while(|&c| is_name_char(c)).count();
                }
                identifiers.push((line, start_column, end_column));
                blank(&mut out, &text[first_line.len()..]);
            }
            Position::Value => {
                // `$1` padded with spaces (`{{` is two characters)
                out.push_str("$1");
                blank(&mut out, &text[2..]);
            }
            Position::Statement => blank(&mut out, text),
        }
        for c in text.chars() {
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        rest = end;
    }
    out.push_str(&sql[rest..]);

    Preprocessed::new(Cow::Owned(out), identifiers)
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

    fn mask(sql: &str) -> String {
        let result = mask_jinja(sql).text.into_owned();
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
            "WHERE d > '$1                '"
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
    fn expressions_in_names_become_identifiers() {
        assert_eq!(
            mask("SELECT 1 AS {{ c }}, total_{{ c }}, {{ c }}_x, t.{{ c }} FROM t"),
            "SELECT 1 AS ___c___, total____c___, ___c____x, t.___c___ FROM t"
        );
        let p = mask_jinja("SELECT total_{{ c }}_x FROM t");
        assert!(p.is_substituted(1, 8));
        assert!(p.is_substituted(1, 22));
        assert!(!p.is_substituted(1, 24));
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
        let p = mask_jinja("SELECT 1\nFROM {{ ref('a') }}.users x");
        assert!(p.is_substituted(2, 6));
        assert!(p.is_substituted(2, 25));
        assert!(!p.is_substituted(2, 5));
        assert!(!p.is_substituted(2, 27));
    }
}
