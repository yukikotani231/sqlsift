//! psql script support (PostgreSQL dialect only)
//!
//! SQL files meant to be run with `psql -f` may contain backslash meta-commands
//! (`\set`, `\i`, `\connect`, `\if`, ...) and variable interpolation (`:var`,
//! `:'var'`, `:"var"`). Neither is SQL, so [`preprocess`] rewrites them into text
//! the parser accepts before parsing:
//!
//! - A meta-command (from an unquoted `\` to the end of its line) is blanked out.
//!   `\g`, `\gx`, `\gset`, `\gexec`, `\gdesc`, `\crosstabview` and `\watch` send the
//!   query buffer, so when a query precedes them the `\` becomes a `;`.
//! - `:var` and `:'var'` become the placeholder `$1` (untyped, like a bind parameter).
//! - `:"var"` (an identifier substitution) and `:var` where a table name is expected
//!   (after FROM, JOIN, INTO, UPDATE, TABLE) become an identifier; name diagnostics
//!   reported on it are dropped with [`Preprocessed::is_substituted`].
//!
//! The rewrite keeps the input's length and line structure byte for byte, so parser
//! and diagnostic locations still point at the original text. `::` casts, `:=`,
//! array slices (`a[i:j]`) and anything inside literals, quoted identifiers and
//! comments are left alone.

use std::borrow::Cow;

/// Source text with psql syntax rewritten into SQL
pub(crate) struct Preprocessed<'a> {
    /// The rewritten text (same length and line breaks as the input)
    pub text: Cow<'a, str>,
    /// Identifier substitutions: (line, first column, end column), 1-indexed and
    /// end-exclusive
    identifiers: Vec<(usize, usize, usize)>,
}

impl Preprocessed<'_> {
    /// The input unchanged
    pub fn unchanged(sql: &str) -> Preprocessed<'_> {
        Preprocessed {
            text: Cow::Borrowed(sql),
            identifiers: Vec::new(),
        }
    }

    /// Whether `line`/`column` is inside an identifier substituted from a psql variable
    pub fn is_substituted(&self, line: usize, column: usize) -> bool {
        self.identifiers
            .iter()
            .any(|&(l, start, end)| l == line && (start..end).contains(&column))
    }
}

/// Meta-commands that send the query buffer to the server, ending the query
const QUERY_TERMINATORS: &[&str] = &["g", "gx", "gset", "gexec", "gdesc", "crosstabview", "watch"];

/// Keywords after which a bare `:var` names a table
const TABLE_KEYWORDS: &[&str] = &["from", "join", "into", "update", "table"];

/// Rewrite psql meta-commands and variable interpolations (see the module docs)
pub(crate) fn preprocess(sql: &str) -> Preprocessed<'_> {
    if !sql.contains(['\\', ':']) {
        return Preprocessed::unchanged(sql);
    }

    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut out = bytes.to_vec();
    let mut identifiers = Vec::new();
    let mut changed = false;

    // Line and character column of `i`, kept up to date as the scan advances
    let mut line = 1;
    let mut line_start = 0;
    let column = |line_start: usize, i: usize| sql[line_start..i].chars().count() + 1;

    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80;
    let is_var_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // Whether the query buffer holds anything since the last `;`
    let mut in_query = false;
    // Nesting of `[ ]`, where `:` is an array slice
    let mut brackets = 0usize;
    // The last word outside literals and comments (for `FROM :tbl`)
    let mut last_word: Option<(usize, usize)> = None;

    let mut i = 0;
    while i < len {
        let b = bytes[i];
        match b {
            b'\n' => {
                line += 1;
                line_start = i + 1;
                i += 1;
                continue;
            }
            _ if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                // Block comments nest in PostgreSQL
                i += 2;
                let mut depth = 1;
                while i < len && depth > 0 {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        i += 2;
                    } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        i += 2;
                    } else {
                        if bytes[i] == b'\n' {
                            line += 1;
                            line_start = i + 1;
                        }
                        i += 1;
                    }
                }
                continue;
            }
            _ => {}
        }

        // Everything else is part of the query
        let was_in_query = in_query;
        in_query = true;
        let word_start = i;

        match b {
            b'\'' | b'"' => {
                // E'...' strings honor backslash escapes
                let backslash = b == b'\''
                    && i > 0
                    && matches!(bytes[i - 1], b'E' | b'e')
                    && (i < 2 || !is_ident_byte(bytes[i - 2]));
                i += 1;
                while i < len {
                    if backslash && bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'\n' {
                        line += 1;
                        line_start = i + 1;
                    }
                    if bytes[i] == b {
                        i += 1;
                        if bytes.get(i) != Some(&b) {
                            break;
                        }
                    }
                    i += 1;
                }
                last_word = None;
            }
            b'$' if i == 0 || !is_ident_byte(bytes[i - 1]) => {
                // Dollar-quoted string: $$...$$ or $tag$...$tag$
                let tag_len = bytes[i + 1..]
                    .iter()
                    .take_while(|&&c| is_var_byte(c))
                    .count();
                let closing = i + 1 + tag_len;
                let starts_with_digit = bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
                if bytes.get(closing) == Some(&b'$') && !starts_with_digit {
                    let tag = &sql[i..=closing];
                    let body = closing + 1;
                    let end = sql[body..].find(tag).map_or(len, |p| body + p + tag.len());
                    for (offset, _) in sql[i..end].match_indices('\n') {
                        line += 1;
                        line_start = i + offset + 1;
                    }
                    i = end;
                } else {
                    i += 1;
                }
                last_word = None;
            }
            b'\\' => {
                // A meta-command, running to the end of the line
                let name_len = bytes[i + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .count();
                let name = &sql[i + 1..i + 1 + name_len];
                let end = sql[i..].find('\n').map_or(len, |p| i + p);
                out[i..end].fill(b' ');
                if was_in_query && QUERY_TERMINATORS.contains(&name) {
                    out[i] = b';';
                }
                in_query = false;
                changed = true;
                i = end;
                last_word = None;
            }
            b':' => {
                let next = bytes.get(i + 1).copied();
                if matches!(next, Some(b':') | Some(b'=')) {
                    // `::` cast, `:=` assignment
                    i += 2;
                    continue;
                }
                let substitution = match next {
                    _ if brackets > 0 => None,
                    Some(quote @ (b'\'' | b'"')) => {
                        let name_len = bytes[i + 2..]
                            .iter()
                            .take_while(|&&c| is_var_byte(c))
                            .count();
                        let close = i + 2 + name_len;
                        (name_len > 0 && bytes.get(close) == Some(&quote))
                            .then_some((close + 1, quote == b'"'))
                    }
                    Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                        let name_len = bytes[i + 1..]
                            .iter()
                            .take_while(|&&c| is_var_byte(c))
                            .count();
                        let table_position = last_word.is_some_and(|(s, e)| {
                            TABLE_KEYWORDS
                                .iter()
                                .any(|k| sql[s..e].eq_ignore_ascii_case(k))
                        });
                        Some((i + 1 + name_len, table_position))
                    }
                    _ => None,
                };
                let Some((end, identifier)) = substitution else {
                    i += 1;
                    last_word = None;
                    continue;
                };
                changed = true;
                if identifier {
                    // An identifier as long as the interpolation: `_name_`
                    for (k, &c) in bytes[i..end].iter().enumerate() {
                        out[i + k] = if is_var_byte(c) { c } else { b'_' };
                    }
                    // Including a qualified name it starts (`:"schema".table`)
                    let mut name_end = end;
                    while name_end < len
                        && (is_var_byte(bytes[name_end]) || matches!(bytes[name_end], b'.' | b'"'))
                    {
                        name_end += 1;
                    }
                    identifiers.push((line, column(line_start, i), column(line_start, name_end)));
                } else {
                    out[i..end].fill(b' ');
                    out[i] = b'$';
                    out[i + 1] = b'1';
                }
                i = end;
                last_word = None;
            }
            _ if is_ident_byte(b) => {
                while i < len && is_ident_byte(bytes[i]) {
                    i += 1;
                }
                last_word = Some((word_start, i));
            }
            _ => {
                match b {
                    b';' => in_query = false,
                    b'[' => brackets += 1,
                    b']' => brackets = brackets.saturating_sub(1),
                    _ => {}
                }
                i += 1;
                last_word = None;
            }
        }
    }

    if !changed {
        return Preprocessed::unchanged(sql);
    }
    // Only ASCII bytes were replaced, and only whole characters, so this is UTF-8
    let text = String::from_utf8(out).unwrap_or_else(|_| sql.to_string());
    Preprocessed {
        text: Cow::Owned(text),
        identifiers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(sql: &str) -> String {
        let result = preprocess(sql).text.into_owned();
        assert_eq!(result.len(), sql.len());
        result
    }

    #[test]
    fn blanks_meta_commands() {
        assert_eq!(rewrite("\\set x 1\nSELECT 1"), "        \nSELECT 1");
        assert_eq!(rewrite("SELECT 1 \\gset\n"), "SELECT 1 ;    \n");
        assert_eq!(rewrite("SELECT 1;\n\\g\n"), "SELECT 1;\n  \n");
        assert_eq!(rewrite("\\echo é"), "        ");
    }

    #[test]
    fn rewrites_variables() {
        assert_eq!(rewrite("x = :'v' AND y = :w"), "x = $1   AND y = $1");
        assert_eq!(rewrite("SELECT :\"c\" FROM :t"), "SELECT __c_ FROM _t");
    }

    #[test]
    fn leaves_sql_alone() {
        for sql in [
            "SELECT a::int, b[1:2], c[i:j], f(x := 1)",
            "SELECT ':a', E'\\':a', \":a\", $$ :a \\g $$, $t$ :a $t$",
            "SELECT 1 -- :a \\g\n/* :a /* \\g */ */",
        ] {
            assert_eq!(rewrite(sql), sql);
        }
    }

    #[test]
    fn records_identifier_positions() {
        let p = preprocess("SELECT 1\nFROM :\"s\".users");
        assert!(p.is_substituted(2, 6));
        assert!(p.is_substituted(2, 15));
        assert!(!p.is_substituted(2, 5));
        assert!(!p.is_substituted(1, 6));
    }
}
