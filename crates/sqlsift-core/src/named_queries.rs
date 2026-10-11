//! aiosql and HugSQL query files
//!
//! [aiosql](https://nackjicholson.github.io/aiosql/) (Python) and
//! [HugSQL](https://www.hugsql.org) (Clojure) load named queries from `.sql` files:
//!
//! ```sql
//! -- name: get-user-by-id^
//! select * from users where id = :id;
//!
//! -- :name get-users-by-ids :? :*
//! select * from users where id in (:v*:ids)
//! ```
//!
//! [`header`] recognises the name comments (aiosql's `-- name: <name>[(params)]<op>`
//! with the operation suffixes `^`, `$`, `!`, `<!`, `*!`, `#`, and HugSQL's
//! `-- :name <name> :<command> :<result>` and `-- :snip <name>`), so that
//! diagnostics say which query they are in (see [`crate::sqlc::QueryNames`]).
//!
//! In a file with such comments, [`mask`] rewrites the parameters before parsing,
//! keeping every line and character column:
//!
//! - A value parameter (aiosql `:name` and `:obj.attr`; HugSQL `:name`, `:v:name`,
//!   `:v*:names`, `:t:name`, `:t*:names`, other `:type:name`) becomes an untyped
//!   placeholder (`$1`, or `?` outside PostgreSQL), `($1)` right after `IN`.
//!   HugSQL's `VALUES :t*:rows` becomes `TABLE  _.<identifier>`: rows of unknown
//!   columns.
//! - A HugSQL identifier parameter (`:i:name`, `:i*:names`, `:identifier:name`)
//!   becomes an identifier made from its text; name diagnostics reported on it are
//!   dropped.
//! - A HugSQL SQL parameter (`:sql:name`, `:snip:name`, `:snip*:name`,
//!   `:frag:name`) is a query fragment: it becomes whitespace, and names the
//!   statement can't resolve are not reported.
//! - The body of a HugSQL snippet (`-- :snip name`) and every query with a Clojure
//!   expression (`--~ ...`, `/*~ ... ~*/`) become whitespace: their SQL is only
//!   known at run time.
//! - Queries need not end with `;`: a name comment starts a new statement (its
//!   `-- ` becomes `;--`, or the space before an indented comment becomes `;`).
//!
//! `::` casts, `:=`, array slices (`a[i:j]`), `a:b` paths and anything inside
//! literals, quoted identifiers and comments are left alone.

use std::borrow::Cow;

use crate::dialect::SqlDialect;
use crate::sqlc::{is_ident_byte, skip_block_comment, skip_dollar_quoted, skip_line, skip_quoted};

/// The library a query file is written for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Aiosql,
    HugSql,
}

/// A query name comment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header<'a> {
    flavor: Flavor,
    /// The query (or snippet) name
    pub name: &'a str,
    /// Whether it names a HugSQL snippet (a fragment, not a statement)
    snippet: bool,
}

/// The aiosql or HugSQL query name comment on `line`, if it is one
pub(crate) fn header(line: &str) -> Option<Header<'_>> {
    let comment = line.trim_start().strip_prefix("--")?;
    hugsql_header(comment).or_else(|| aiosql_header(comment))
}

/// aiosql: `name: <name>[(<params>)]<op>` (after `--`)
fn aiosql_header(comment: &str) -> Option<Header<'_>> {
    let rest = comment
        .trim_start()
        .strip_prefix("name")?
        .trim_start()
        .strip_prefix(':')?
        .trim();
    let name_len = rest
        .char_indices()
        .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    let name = &rest[..name_len];
    let mut rest = &rest[name_len..];
    if let Some(params) = rest.strip_prefix('(') {
        let close = params.find(')')?;
        let valid = params[..close]
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ',' | ' ' | '\t'));
        if !valid {
            return None;
        }
        rest = &params[close + 1..];
    }
    let valid = !name.is_empty()
        && !name.starts_with('-')
        && ["", "^", "$", "!", "<!", "*!", "#"].contains(&rest);
    valid.then_some(Header {
        flavor: Flavor::Aiosql,
        name,
        snippet: false,
    })
}

/// HugSQL: `:name <name> [:<command>] [:<result>]`, `:name-`, `:snip`, `:snip-`
/// (after `--`)
fn hugsql_header(comment: &str) -> Option<Header<'_>> {
    let mut words = comment.split_whitespace();
    let snippet = match words.next()? {
        ":name" | ":name-" => false,
        ":snip" | ":snip-" => true,
        _ => return None,
    };
    let name = words.next().filter(|n| !n.starts_with(':'))?;
    words.all(|w| w.starts_with(':')).then_some(Header {
        flavor: Flavor::HugSql,
        name,
        snippet,
    })
}

/// Text with aiosql / HugSQL parameters rewritten into SQL (see the module docs)
pub(crate) struct Masked<'a> {
    /// The rewritten text (same lines and character columns as the input)
    pub text: Cow<'a, str>,
    /// Identifier parameters: (line, first column, end column), 1-indexed and
    /// end-exclusive
    pub identifiers: Vec<(usize, usize, usize)>,
    /// SQL parameters dropped as query fragments: (line, column, whether it may join
    /// relations)
    pub fragments: Vec<(usize, usize, bool)>,
}

/// Rewrite the parameters of an aiosql or HugSQL query file (see the module docs);
/// any other file is returned unchanged
pub(crate) fn mask(sql: &str, dialect: SqlDialect) -> Masked<'_> {
    let unchanged = |sql| Masked {
        text: Cow::Borrowed(sql),
        identifiers: Vec::new(),
        fragments: Vec::new(),
    };
    if !sql.contains("name") && !sql.contains(":snip") {
        return unchanged(sql);
    }
    let headers: Vec<(usize, Header)> = sql
        .lines()
        .enumerate()
        .filter_map(|(i, line)| Some((i, header(line)?)))
        .collect();
    let flavor = if headers.iter().any(|(_, h)| h.flavor == Flavor::HugSql) {
        Flavor::HugSql
    } else if headers.is_empty() {
        return unchanged(sql);
    } else {
        Flavor::Aiosql
    };

    let text = if flavor == Flavor::HugSql {
        blank_dynamic_sections(sql, &headers)
    } else {
        sql.to_string()
    };
    let line_starts = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut masker = Masker {
        out: text.into_bytes(),
        line_starts,
        flavor,
        dialect,
        identifiers: Vec::new(),
        fragments: Vec::new(),
    };
    masker.run();
    // Only ASCII was replaced by ASCII, so this is still UTF-8
    let text = String::from_utf8(masker.out).unwrap_or_else(|_| sql.to_string());
    Masked {
        text: Cow::Owned(text),
        identifiers: masker.identifiers,
        fragments: masker.fragments,
    }
}

/// `sql` with the bodies of HugSQL snippets and of queries with Clojure expressions
/// blanked (every character but line breaks becomes a space)
fn blank_dynamic_sections(sql: &str, headers: &[(usize, Header)]) -> String {
    let lines: Vec<&str> = sql.split_inclusive('\n').collect();
    let mut out = String::with_capacity(sql.len());
    let mut blank = false;
    let mut next_header = headers.iter().peekable();
    for (i, line) in lines.iter().enumerate() {
        if let Some((_, header)) = next_header.next_if(|(h, _)| *h == i) {
            let end = next_header.peek().map_or(lines.len(), |(h, _)| *h);
            blank = header.snippet
                || lines[i + 1..end]
                    .iter()
                    .any(|l| l.contains("--~") || l.contains("/*~"));
            out.push_str(line);
        } else if blank {
            out.extend(
                line.chars()
                    .map(|c| if matches!(c, '\n' | '\r') { c } else { ' ' }),
            );
        } else {
            out.push_str(line);
        }
    }
    out
}

/// What a HugSQL parameter stands for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parameter {
    Value,
    Tuples,
    Identifier,
    Fragment,
}

struct Masker {
    out: Vec<u8>,
    /// Byte index where each line starts (rewriting never moves a line break)
    line_starts: Vec<usize>,
    flavor: Flavor,
    dialect: SqlDialect,
    identifiers: Vec<(usize, usize, usize)>,
    fragments: Vec<(usize, usize, bool)>,
}

impl Masker {
    fn run(&mut self) {
        let postgres = self.dialect == SqlDialect::PostgreSQL;
        let mysql = self.dialect == SqlDialect::MySQL;
        let len = self.out.len();
        // Nesting of `[ ]`, where `:` is an array slice
        let mut brackets = 0usize;
        // Nesting of `( )` in the current statement: a name comment inside
        // parentheses (`CREATE TABLE t (\n-- name: text\n...`) is just a comment
        let mut parens = 0usize;
        let mut i = 0;
        while i < len {
            let bytes = &self.out;
            let b = bytes[i];
            let prev = i.checked_sub(1).map(|p| bytes[p]);
            match b {
                b'-' if bytes.get(i + 1) == Some(&b'-') => {
                    let end = skip_line(bytes, i);
                    if parens == 0 {
                        self.end_statement_before_header(i, end);
                    }
                    i = end;
                }
                b'#' if mysql => i = skip_line(bytes, i),
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i = skip_block_comment(bytes, i, postgres);
                }
                b'\'' | b'"' | b'`' => {
                    let backslash = b != b'`'
                        && (mysql
                            || postgres
                                && b == b'\''
                                && matches!(prev, Some(b'E' | b'e'))
                                && (i < 2 || !is_ident_byte(bytes[i - 2])));
                    i = skip_quoted(bytes, i, backslash);
                }
                b'$' if postgres && !prev.is_some_and(is_ident_byte) => {
                    i = skip_dollar_quoted(bytes, i);
                }
                b'(' => {
                    parens += 1;
                    i += 1;
                }
                b')' => {
                    parens = parens.saturating_sub(1);
                    i += 1;
                }
                b';' => {
                    parens = 0;
                    i += 1;
                }
                b'[' => {
                    brackets += 1;
                    i += 1;
                }
                b']' => {
                    brackets = brackets.saturating_sub(1);
                    i += 1;
                }
                b':' if brackets == 0 && !prev.is_some_and(|p| is_ident_byte(p) || p == b':') => {
                    i = self.parameter(i).unwrap_or(i + 1);
                }
                _ => i += 1,
            }
        }
    }

    /// A query name comment from `start` to `end` starts a new statement: put a `;`
    /// before it
    fn end_statement_before_header(&mut self, start: usize, end: usize) {
        let line = self.line_starts.partition_point(|&s| s <= start);
        let line_start = self.line_starts[line - 1];
        if !self.out[line_start..start]
            .iter()
            .all(|&c| c == b' ' || c == b'\t')
        {
            return;
        }
        let is_header = std::str::from_utf8(&self.out[start..end])
            .ok()
            .and_then(header)
            .is_some_and(|h| h.flavor == self.flavor);
        if !is_header {
            return;
        }
        if start > line_start {
            self.out[start - 1] = b';';
        } else if matches!(self.out.get(start + 2), Some(b' ' | b'\t')) {
            self.out[start..start + 3].copy_from_slice(b";--");
        }
    }

    /// Rewrite the parameter starting with the `:` at `start`; the index after it
    fn parameter(&mut self, start: usize) -> Option<usize> {
        let bytes = &self.out;
        let (kind, name_start) = match self.flavor {
            Flavor::Aiosql => (Parameter::Value, start + 1),
            Flavor::HugSql => hugsql_type(bytes, start + 1),
        };
        let first = *bytes.get(name_start)?;
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return None;
        }
        let end = match self.flavor {
            Flavor::Aiosql => aiosql_name_end(bytes, name_start),
            Flavor::HugSql => hugsql_name_end(bytes, name_start),
        };

        let width = end - start;
        let placeholder: &[u8] = if self.dialect == SqlDialect::PostgreSQL {
            b"$1"
        } else {
            b"?"
        };
        let word = previous_word(bytes, start);
        let after = |keyword: &[u8]| word.is_some_and(|(_, w)| w.eq_ignore_ascii_case(keyword));
        let (after_values, after_in) = (after(b"values"), after(b"in"));
        let word_start = word.map_or(0, |(s, _)| s);
        match kind {
            Parameter::Tuples if after_values => {
                // `VALUES :t*:rows`: `TABLE  _.<identifier>`, rows of unknown columns
                self.out[word_start..word_start + 6].copy_from_slice(b"TABLE ");
                self.write_identifier(start, end);
                self.out[start..start + 2].copy_from_slice(b"_.");
            }
            Parameter::Identifier => self.write_identifier(start, end),
            Parameter::Fragment => {
                self.out[start..end].fill(b' ');
                let (line, column) = self.position(start);
                self.fragments.push((line, column, true));
            }
            Parameter::Value | Parameter::Tuples => {
                self.out[start..end].fill(b' ');
                if after_in && width >= placeholder.len() + 2 {
                    self.out[start] = b'(';
                    self.out[start + 1..=start + placeholder.len()].copy_from_slice(placeholder);
                    self.out[start + placeholder.len() + 1] = b')';
                } else {
                    self.out[start..start + placeholder.len()].copy_from_slice(placeholder);
                }
            }
        }
        Some(end)
    }

    /// Replace `start..end` with an identifier made from its text, and record it
    fn write_identifier(&mut self, start: usize, end: usize) {
        for c in &mut self.out[start..end] {
            if !(c.is_ascii_alphanumeric() || *c == b'_') {
                *c = b'_';
            }
        }
        let (line, column) = self.position(start);
        self.identifiers
            .push((line, column, column + (end - start)));
    }

    /// Line and character column (1-indexed) of byte `i`
    fn position(&self, i: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|&start| start <= i);
        let line_start = self.line_starts[line - 1];
        let column = String::from_utf8_lossy(&self.out[line_start..i])
            .chars()
            .count()
            + 1;
        (line, column)
    }
}

/// The kind of the HugSQL parameter whose text (after `:`) starts at `i`, and where
/// its name starts: `:v*:ids` is a value list named `ids`, `:id` a value named `id`
fn hugsql_type(bytes: &[u8], i: usize) -> (Parameter, usize) {
    let word = bytes[i..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let star = usize::from(bytes.get(i + word) == Some(&b'*'));
    let colon = i + word + star;
    let typed = word > 0
        && bytes.get(colon) == Some(&b':')
        && bytes
            .get(colon + 1)
            .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_');
    if !typed {
        return (Parameter::Value, i);
    }
    let kind = match &bytes[i..i + word] {
        b"i" | b"identifier" => Parameter::Identifier,
        b"sql" | b"snip" | b"frag" => Parameter::Fragment,
        b"t" | b"tuple" if star == 1 => Parameter::Tuples,
        _ => Parameter::Value,
    };
    (kind, colon + 1)
}

/// End of an aiosql parameter name (`id`, `user.id`) starting at `i`
fn aiosql_name_end(bytes: &[u8], mut i: usize) -> usize {
    let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_' || *c >= 0x80;
    loop {
        i += bytes[i..].iter().take_while(|c| word(c)).count();
        if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(word) {
            i += 1;
        } else {
            return i;
        }
    }
}

/// End of a HugSQL parameter name (a Clojure keyword: `id`, `user-id`,
/// `user.id`, `ns/id`, `valid?`) starting at `i`
fn hugsql_name_end(bytes: &[u8], i: usize) -> usize {
    let mut end = i + bytes[i..]
        .iter()
        .take_while(|&&c| {
            c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'/' | b'?' | b'!')
        })
        .count();
    // `:id.` ending a sentence, `:a-` before a number
    while matches!(bytes[end - 1], b'.' | b'-' | b'/') {
        end -= 1;
    }
    end
}

/// The word (letters only) right before `i`, skipping whitespace: (start, word)
fn previous_word(bytes: &[u8], i: usize) -> Option<(usize, &[u8])> {
    let end = bytes[..i]
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map(|p| p + 1)?;
    let start = bytes[..end]
        .iter()
        .rposition(|c| !c.is_ascii_alphabetic())
        .map_or(0, |p| p + 1);
    let word = &bytes[start..end];
    let whole = start == 0 || !is_ident_byte(bytes[start - 1]);
    (!word.is_empty() && whole).then_some((start, word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(sql: &str, dialect: SqlDialect) -> String {
        let masked = mask(sql, dialect);
        assert_eq!(masked.text.len(), sql.len());
        masked.text.into_owned()
    }

    fn name(line: &str) -> Option<&str> {
        header(line).map(|h| h.name)
    }

    #[test]
    fn parses_aiosql_headers() {
        assert_eq!(name("-- name: get-user-by-id^"), Some("get-user-by-id"));
        assert_eq!(name("--name:get_all"), Some("get_all"));
        assert_eq!(name("-- name: count_users$  "), Some("count_users"));
        assert_eq!(name("-- name : publish_blog<!"), Some("publish_blog"));
        assert_eq!(name("-- name: bulk_insert*!"), Some("bulk_insert"));
        assert_eq!(name("-- name: create_schema#"), Some("create_schema"));
        assert_eq!(name("-- name: delete!"), Some("delete"));
        assert_eq!(name("-- name: get_by(id, name)^"), Some("get_by"));
        assert_eq!(name("-- name: the user's name"), None);
        assert_eq!(name("-- name: GetUser :one"), None);
        assert_eq!(name("-- name: x?"), None);
        assert_eq!(name("select 1 -- name: x"), None);
    }

    #[test]
    fn parses_hugsql_headers() {
        assert_eq!(name("-- :name get-user :? :1"), Some("get-user"));
        assert_eq!(name("--:name insert-user! :! :n"), Some("insert-user!"));
        assert_eq!(name("-- :name- private-q"), Some("private-q"));
        let snip = header("-- :snip where-id").unwrap();
        assert_eq!((snip.name, snip.snippet), ("where-id", true));
        assert_eq!(name("-- :name :? :1"), None);
        assert_eq!(name("-- :doc Get a user"), None);
        assert_eq!(name("-- :name get it"), None);
    }

    #[test]
    fn leaves_other_files_alone() {
        let sql = "SELECT * FROM t WHERE a = :a";
        assert_eq!(rewrite(sql, SqlDialect::PostgreSQL), sql);
        let sqlc = "-- name: GetUser :one\nSELECT * FROM t WHERE a = :a";
        assert_eq!(rewrite(sqlc, SqlDialect::PostgreSQL), sqlc);
    }

    #[test]
    fn masks_aiosql_parameters() {
        assert_eq!(
            rewrite(
                "-- name: q^\nselect * from t where a = :a and b=:u.id and c::text = :c_1;",
                SqlDialect::PostgreSQL
            ),
            ";--name: q^\nselect * from t where a = $1 and b=$1    and c::text = $1  ;"
        );
        assert_eq!(
            rewrite("-- name: q\nselect :a, x in :ids", SqlDialect::SQLite),
            ";--name: q\nselect ? , x in (?) "
        );
    }

    #[test]
    fn leaves_non_parameters_alone() {
        let sql = "-- name: q\nselect a::int, b[1:n], c:d, ':a', \":a\", $$ :a $$, x := 1 -- :a\n/* :a */";
        let masked = rewrite(sql, SqlDialect::PostgreSQL);
        assert_eq!(&masked[3..], &sql[3..]);
    }

    #[test]
    fn masks_hugsql_parameters() {
        let pg = SqlDialect::PostgreSQL;
        assert_eq!(
            rewrite(
                "-- :name q :? :*\nselect * from t where id in (:v*:ids) and n = :user-name and m = :id::int",
                pg
            ),
            ";--:name q :? :*\nselect * from t where id in ($1     ) and n = $1         and m = $1 ::int"
        );
        let masked = mask(
            "-- :name q :? :*\nselect :i*:cols from t order by :i:col",
            pg,
        );
        assert_eq!(
            masked.text,
            ";--:name q :? :*\nselect _i__cols from t order by _i_col"
        );
        assert_eq!(masked.identifiers, vec![(2, 8, 16), (2, 33, 39)]);
        let masked = mask("-- :name q :? :*\nselect * from t :snip:where", pg);
        assert_eq!(masked.text, ";--:name q :? :*\nselect * from t            ");
        assert_eq!(masked.fragments, vec![(2, 17, true)]);
        assert_eq!(
            rewrite(
                "-- :name q :! :n\ninsert into t (a, b) values :t*:rows",
                SqlDialect::MySQL
            ),
            ";--:name q :! :n\ninsert into t (a, b) TABLE  _.__rows"
        );
    }

    #[test]
    fn blanks_snippets_and_clojure_expressions() {
        let sql = "-- :snip where-id\nwhere id = :id\n-- :name q :? :*\nselect * from t\n--~ (when (:id params) \"where id = :id\")\n-- :name r :? :*\nselect 1";
        assert_eq!(
            rewrite(sql, SqlDialect::PostgreSQL),
            format!(
                ";--:snip where-id\n{}\n;--:name q :? :*\n{}\n{}\n;--:name r :? :*\nselect 1",
                " ".repeat(14),
                " ".repeat(15),
                " ".repeat(40)
            )
        );
    }

    #[test]
    fn leaves_name_comments_in_parentheses_alone() {
        let sql = "create table t (\n-- name: text\nname text\n);\n-- name: q\nselect 1";
        assert_eq!(
            rewrite(sql, SqlDialect::PostgreSQL),
            sql.replace("\n-- name: q", "\n;--name: q")
        );
    }

    #[test]
    fn ends_statements_at_indented_headers() {
        assert_eq!(
            rewrite("select 1\n  -- name: q\nselect 2", SqlDialect::PostgreSQL),
            "select 1\n ;-- name: q\nselect 2"
        );
    }
}
