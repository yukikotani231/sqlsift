//! CREATE TABLE clauses of the data warehouse dialects that sqlparser can't parse
//! and that don't change the table's columns: Redshift's distribution, sort key and
//! compression attributes, Databricks' table options and complex column types.
//!
//! [`mask_table_attributes`] finds them in a tokenized CREATE TABLE statement so
//! that [`super::builder::mask_unsupported_clauses`] can blank them out (keeping
//! every other character in place) and parse the statement again.

use std::ops::Range;

use sqlparser::tokenizer::{Token, TokenWithSpan};

use crate::dialect::SqlDialect;

/// A change to the statement text: blank `range` (bytes), then write `text` at its
/// start (`text` is no longer than the range)
pub(super) struct Edit {
    pub range: Range<usize>,
    pub text: &'static str,
}

/// Mark the tokens (indexes into `tokens`) of `dialect`'s table attributes in
/// `drop`, and return the edits that aren't whole tokens. `significant` are the
/// indexes of the non-whitespace tokens of the CREATE statement; `offset` gives a
/// token's byte offset in the statement text.
pub(super) fn mask_table_attributes(
    dialect: SqlDialect,
    tokens: &[TokenWithSpan],
    significant: &[usize],
    offset: &dyn Fn(&TokenWithSpan) -> Option<usize>,
    drop: &mut [bool],
) -> Vec<Edit> {
    let sig: Vec<&Token> = significant.iter().map(|&i| &tokens[i].token).collect();
    // Only the table definition: stop at the query of a CREATE TABLE ... AS
    let end = definition_end(&sig);
    let mut edits = Vec::new();
    let drop_range = |range: Range<usize>, drop: &mut [bool]| {
        for n in range {
            drop[significant[n]] = true;
        }
    };
    match dialect {
        SqlDialect::Redshift => {
            let mut n = 0;
            while n < end {
                let after_separator = n == 0 || matches!(sig[n - 1], Token::LParen | Token::Comma);
                let next = n + 1;
                let skip = match keyword(sig[n]).as_deref() {
                    // DISTSTYLE ALL / ENCODE zstd / BACKUP NO
                    Some("DISTSTYLE" | "ENCODE" | "BACKUP")
                        if !after_separator && next < end && is_word(sig[next]) =>
                    {
                        Some(n..next + 1)
                    }
                    // [COMPOUND | INTERLEAVED] SORTKEY (a, b) / SORTKEY AUTO /
                    // DISTKEY (a), or a column's DISTKEY / SORTKEY
                    Some(kw @ ("SORTKEY" | "DISTKEY" | "COMPOUND" | "INTERLEAVED"))
                        if !after_separator =>
                    {
                        let key = if matches!(kw, "COMPOUND" | "INTERLEAVED") {
                            let sortkey = sig.get(next).and_then(|t| keyword(t));
                            (sortkey.as_deref() == Some("SORTKEY")).then_some(next)
                        } else {
                            Some(n)
                        };
                        key.map(|key| match sig.get(key + 1) {
                            Some(Token::LParen) => {
                                n..matching_paren(&sig, key + 1).map_or(key + 1, |close| close + 1)
                            }
                            Some(t) if keyword(t).as_deref() == Some("AUTO") => n..key + 2,
                            _ => n..key + 1,
                        })
                    }
                    // A column's IDENTITY(seed, step): sqlparser takes a bare IDENTITY
                    // (and drops it), so keep the column generated with a default;
                    // GENERATED ... AS IDENTITY(seed, step) loses its parameters
                    Some("IDENTITY") if matches!(sig.get(next), Some(Token::LParen)) => {
                        if let Some(close) = matching_paren(&sig, next) {
                            if n > 0 && keyword(sig[n - 1]).as_deref() == Some("AS") {
                                drop_range(next..close + 1, drop);
                            } else if let (Some(start), Some(stop)) = (
                                offset(&tokens[significant[n]]),
                                offset(&tokens[significant[close]]),
                            ) {
                                edits.push(Edit {
                                    range: start..stop + 1,
                                    text: "DEFAULT 0",
                                });
                            }
                            n = close + 1;
                            continue;
                        }
                        None
                    }
                    _ => None,
                };
                match skip {
                    Some(range) if !range.is_empty() => {
                        n = range.end;
                        drop_range(range, drop);
                    }
                    _ => n += 1,
                }
            }
        }
        SqlDialect::Databricks => {
            // Table options after the column list
            // (the first `(` that doesn't start an option's list)
            let columns_end = (3..end)
                .find(|&n| {
                    sig[n] == &Token::LParen
                        && !matches!(
                            keyword(sig[n - 1]).as_deref(),
                            Some("BY" | "OPTIONS" | "TBLPROPERTIES")
                        )
                })
                .and_then(|open| matching_paren(&sig, open))
                .unwrap_or(0);
            let mut n = 0;
            while n < end {
                let next = n + 1;
                let skip = match keyword(sig[n]).as_deref() {
                    // USING DELTA
                    Some("USING") if n > columns_end && next < end && is_word(sig[next]) => {
                        Some(n..next + 1)
                    }
                    // CLUSTER BY (a, b) / OPTIONS (k = v) / PARTITIONED BY (a, b) on
                    // columns of the table (PARTITIONED BY (c TYPE) adds columns and
                    // parses)
                    Some(kw @ ("CLUSTER" | "OPTIONS" | "PARTITIONED")) if n > columns_end => {
                        let open = if kw == "OPTIONS" { next } else { next + 1 };
                        let by_ok = kw == "OPTIONS"
                            || keyword(sig.get(next).copied().unwrap_or(&Token::EOF)).as_deref()
                                == Some("BY");
                        match (by_ok, sig.get(open)) {
                            (true, Some(Token::LParen)) => {
                                matching_paren(&sig, open).and_then(|close| {
                                    // `(a, b)`: one name per item
                                    let names_only =
                                        sig[open + 1..close].iter().enumerate().all(|(k, t)| {
                                            if k % 2 == 0 {
                                                is_word(t)
                                            } else {
                                                *t == &Token::Comma
                                            }
                                        });
                                    (kw != "PARTITIONED" || names_only).then_some(n..close + 1)
                                })
                            }
                            _ => None,
                        }
                    }
                    // STRUCT<a: T, ...> column types: the type arguments. sqlparser
                    // parses MAP<K, V> (a bare MAP doesn't), so it is kept whole
                    Some(kw @ ("MAP" | "STRUCT"))
                        if n < columns_end && sig.get(next) == Some(&&Token::Lt) =>
                    {
                        let close = matching_angle(tokens, significant, next, offset);
                        if let (Some(start), Some(stop), "STRUCT") = (
                            offset(&tokens[significant[next]]),
                            close.as_ref().map(|c| c.1),
                            kw,
                        ) {
                            edits.push(Edit {
                                range: start..stop + 1,
                                text: "",
                            });
                        }
                        n = close.map_or(next, |c| c.0) + 1;
                        continue;
                    }
                    _ => None,
                };
                match skip {
                    Some(range) => {
                        n = range.end;
                        drop_range(range, drop);
                    }
                    None => n += 1,
                }
            }
        }
        _ => {}
    }
    edits
}

/// Uppercase value of an unquoted word
fn keyword(token: &Token) -> Option<String> {
    match token {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    }
}

fn is_word(token: &Token) -> bool {
    matches!(token, Token::Word(_))
}

/// Index (into `sig`) of the end of a CREATE TABLE's definition: its top-level `AS`
/// (CREATE TABLE ... AS SELECT), or the end
fn definition_end(sig: &[&Token]) -> usize {
    let mut depth = 0usize;
    for (n, token) in sig.iter().enumerate() {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => depth = depth.saturating_sub(1),
            _ if depth == 0 && keyword(token).as_deref() == Some("AS") => return n,
            _ => {}
        }
    }
    sig.len()
}

/// Index of the `)` matching the `(` at `open`
fn matching_paren(sig: &[&Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (n, token) in sig.iter().enumerate().skip(open) {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(n);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `>` matching the `<` at significant token `open`: the index of the token
/// that holds it and its byte offset (`>>` closes two)
fn matching_angle(
    tokens: &[TokenWithSpan],
    significant: &[usize],
    open: usize,
    offset: &dyn Fn(&TokenWithSpan) -> Option<usize>,
) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    for (n, &i) in significant.iter().enumerate().skip(open) {
        match tokens[i].token {
            Token::Lt => depth += 1,
            Token::Gt => {
                depth -= 1;
                if depth == 0 {
                    return Some((n, offset(&tokens[i])?));
                }
            }
            Token::ShiftRight => {
                if depth <= 2 {
                    // The first `>` closes the type when depth is 1
                    let at = offset(&tokens[i])? + usize::from(depth == 2);
                    return Some((n, at));
                }
                depth -= 2;
            }
            Token::SemiColon => return None,
            _ => {}
        }
    }
    None
}
