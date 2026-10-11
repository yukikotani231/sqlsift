//! Snowflake statements sqlparser can't parse, handled before analysis (alpha)
//!
//! - Snowflake Scripting blocks (`DECLARE ... BEGIN ... END;`, `BEGIN ... END;`)
//!   are one statement each, and are skipped without a parse error
//! - administration statements (`ALTER SESSION`, `ALTER WAREHOUSE`, `CREATE TASK`,
//!   `GRANT ... ON WAREHOUSE`, ...) are skipped without a parse error: they don't
//!   read tables, so nothing in them is checked
//! - positional column references qualified with a table alias (`t.$1`) are
//!   blanked to `$1`, which sqlparser does parse

use sqlparser::dialect::Dialect;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer};

/// Uppercase value of an unquoted word
fn keyword(token: &Token) -> Option<String> {
    match token {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    }
}

/// Byte ranges of the Snowflake Scripting blocks in `sql`: a statement starting
/// with `DECLARE` or `BEGIN` (not `BEGIN TRANSACTION`) up to its matching `END`
/// and the `;` after it. A block whose end isn't found is not returned.
/// `offset` converts a token location to a byte offset in `sql`.
pub(super) fn scripting_blocks(
    dialect: &dyn Dialect,
    sql: &str,
    offset: &dyn Fn(Location) -> usize,
) -> Vec<std::ops::Range<usize>> {
    let Ok(tokens) = Tokenizer::new(dialect, sql).tokenize_with_location() else {
        return Vec::new();
    };
    let sig: Vec<&TokenWithSpan> = tokens
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    let kw = |n: usize| sig.get(n).and_then(|t| keyword(&t.token));

    let mut blocks = Vec::new();
    let mut at_start = true;
    let mut n = 0;
    while n < sig.len() {
        if at_start {
            let opens = match kw(n).as_deref() {
                Some("DECLARE") => true,
                Some("BEGIN") => {
                    !matches!(
                        sig.get(n + 1).map(|t| &t.token),
                        None | Some(Token::SemiColon | Token::EOF)
                    ) && !matches!(kw(n + 1).as_deref(), Some("TRANSACTION" | "WORK" | "NAME"))
                }
                _ => false,
            };
            if opens {
                if let Some(end) = block_end(&sig, n) {
                    blocks.push(offset(sig[n].span.start)..offset(sig[end].span.end));
                    n = end + 1;
                    continue;
                }
            }
        }
        at_start = sig[n].token == Token::SemiColon;
        n += 1;
    }
    blocks
}

/// Index of the last token of the block starting at `start`: the `;` after the
/// `END` that closes its first `BEGIN`, or that `END`
fn block_end(sig: &[&TokenWithSpan], start: usize) -> Option<usize> {
    let kw = |n: usize| sig.get(n).and_then(|t| keyword(&t.token));
    let mut depth = 0usize;
    let mut seen_begin = false;
    let mut n = start;
    while n < sig.len() {
        match kw(n).as_deref() {
            Some("BEGIN") => {
                depth += 1;
                seen_begin = true;
            }
            // CASE expression (`CASE ... END`) or statement (`CASE ... END CASE`)
            Some("CASE") => depth += 1,
            Some("END") => {
                match kw(n + 1).as_deref() {
                    // END IF / END FOR / ...: their openers aren't counted
                    Some("IF" | "FOR" | "WHILE" | "LOOP" | "REPEAT") => {
                        n += 2;
                        continue;
                    }
                    Some("CASE") => n += 1,
                    _ => {}
                }
                depth = depth.checked_sub(1)?;
                if depth == 0 && seen_begin {
                    return Some(match sig.get(n + 1).map(|t| &t.token) {
                        Some(Token::SemiColon) => n + 1,
                        _ => n,
                    });
                }
            }
            _ => {}
        }
        n += 1;
    }
    None
}

/// Whether a statement that doesn't parse is a Snowflake statement sqlsift doesn't
/// analyze: an administration statement (session, warehouse, task, user, stage,
/// grants, ...), as opposed to a query or a table or view definition
pub(super) fn is_unanalyzed_statement(dialect: &dyn Dialect, text: &str) -> bool {
    let Ok(tokens) = Tokenizer::new(dialect, text).tokenize() else {
        return false;
    };
    let words: Vec<String> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .map(|t| keyword(t).unwrap_or_default())
        .collect();
    let word = |n: usize| words.get(n).map_or("", String::as_str);
    match word(0) {
        "ALTER" | "CREATE" | "DROP" | "UNDROP" | "DESCRIBE" | "DESC" | "SHOW" => {
            // The object type, after CREATE's modifiers
            let mut n = 1;
            while matches!(
                word(n),
                "OR" | "REPLACE"
                    | "TEMP"
                    | "TEMPORARY"
                    | "TRANSIENT"
                    | "VOLATILE"
                    | "LOCAL"
                    | "GLOBAL"
                    | "SECURE"
                    | "RECURSIVE"
                    | "MATERIALIZED"
                    | "DYNAMIC"
                    | "HYBRID"
                    | "ICEBERG"
                    | "EXTERNAL"
                    | "EVENT"
            ) {
                n += 1;
            }
            !matches!(word(n), "TABLE" | "VIEW" | "")
        }
        "GRANT" | "REVOKE" | "USE" | "CALL" | "PUT" | "GET" | "LIST" | "LS" | "REMOVE" | "RM"
        | "EXECUTE" | "COMMENT" | "UNSET" | "SET" => true,
        _ => false,
    }
}

/// `text` with the qualifiers of positional column references blanked
/// (`t.$1` -> `  $1`, keeping every other character in place), or `None` if it
/// has none
pub(super) fn mask_positional_columns(dialect: &dyn Dialect, text: &str) -> Option<String> {
    let tokens = Tokenizer::new(dialect, text)
        .tokenize_with_location()
        .ok()?;
    let is_positional = |t: &Token| matches!(t, Token::Placeholder(p) if p.len() > 1 && p.starts_with('$') && p[1..].bytes().all(|b| b.is_ascii_digit()));
    // Character columns to blank, per line: (line, first column, end column)
    let mut blank: Vec<(u64, u64, u64)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !is_positional(&token.token) || i < 2 || tokens[i - 1].token != Token::Period {
            continue;
        }
        // The qualifier: `a.b.` before the placeholder, without whitespace
        let mut first = i;
        let mut period = i - 1;
        while tokens[period].token == Token::Period
            && period >= 1
            && matches!(tokens[period - 1].token, Token::Word(_))
        {
            first = period - 1;
            if period < 2 {
                break;
            }
            period -= 2;
        }
        if first == i {
            continue;
        }
        let (start, end) = (tokens[first].span.start, token.span.start);
        if start.line != end.line {
            continue;
        }
        blank.push((start.line, start.column, end.column));
    }
    if blank.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    for (line_index, line) in text.split_inclusive('\n').enumerate() {
        let line_no = line_index as u64 + 1;
        for (col_index, c) in line.chars().enumerate() {
            let col = col_index as u64 + 1;
            let blanked = blank
                .iter()
                .any(|&(l, from, to)| l == line_no && (from..to).contains(&col));
            out.push(if blanked { ' ' } else { c });
        }
    }
    Some(out)
}
