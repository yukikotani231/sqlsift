//! The grammar: recursive descent over [`Parser`], following PostgreSQL's `gram.y` for the
//! constructs it covers. Anything else is an `OpaqueStmt` (a statement it doesn't know) or
//! an `Error` node (a statement it knows that went wrong).

mod expr;
mod select;
mod types;

use crate::parser::Parser;
use crate::SyntaxKind;

pub(crate) use expr::expr;
pub(crate) use select::select_stmt;

/// PostgreSQL's reserved keywords: never a column or table name without quotes.
const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "both",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "constraint",
    "create",
    "current_catalog",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "default",
    "deferrable",
    "desc",
    "distinct",
    "do",
    "else",
    "end",
    "except",
    "false",
    "fetch",
    "for",
    "foreign",
    "from",
    "grant",
    "group",
    "having",
    "in",
    "initially",
    "intersect",
    "into",
    "lateral",
    "leading",
    "limit",
    "localtime",
    "localtimestamp",
    "not",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "placing",
    "primary",
    "references",
    "returning",
    "select",
    "session_user",
    "some",
    "symmetric",
    "system_user",
    "table",
    "then",
    "to",
    "trailing",
    "true",
    "union",
    "unique",
    "user",
    "using",
    "variadic",
    "when",
    "where",
    "window",
    "with",
];

/// Keywords that can be a function or type name but not a column or table name
/// (`type_func_name_keyword` in `gram.y`).
const TYPE_FUNC_NAME: &[&str] = &[
    "authorization",
    "binary",
    "collation",
    "concurrently",
    "cross",
    "current_schema",
    "freeze",
    "full",
    "ilike",
    "inner",
    "is",
    "isnull",
    "join",
    "left",
    "like",
    "natural",
    "notnull",
    "outer",
    "overlaps",
    "right",
    "similar",
    "tablesample",
    "verbose",
];

fn is_reserved(word: &str) -> bool {
    RESERVED.iter().any(|kw| word.eq_ignore_ascii_case(kw))
}

fn is_type_func_name(word: &str) -> bool {
    TYPE_FUNC_NAME
        .iter()
        .any(|kw| word.eq_ignore_ascii_case(kw))
}

/// Whether the n-th token can be a column or table name (`ColId`): an identifier that is
/// not a reserved or type/function-name keyword, or a quoted identifier.
fn nth_is_col_id(p: &mut Parser<'_>, n: usize) -> bool {
    match p.nth(n) {
        SyntaxKind::QuotedIdent => true,
        SyntaxKind::Ident => {
            let word = p.nth_text(n);
            !is_reserved(word) && !is_type_func_name(word)
        }
        _ => false,
    }
}

fn at_col_id(p: &mut Parser<'_>) -> bool {
    nth_is_col_id(p, 0)
}

/// Whether the current token starts a query (`SELECT ...`).
fn at_query_start(p: &mut Parser<'_>) -> bool {
    p.at_kw("select")
}

pub(crate) fn source_file(p: &mut Parser<'_>) {
    let m = p.start();
    loop {
        while p.eat(SyntaxKind::Semicolon) {}
        if p.at_end() {
            break;
        }
        statement(p);
        if !p.at_end() && !p.at(SyntaxKind::Semicolon) {
            p.error("expected ;");
            let e = p.start();
            skip_balanced_until(p, |p| p.at(SyntaxKind::Semicolon));
            e.complete(p, SyntaxKind::Error);
        }
    }
    m.complete(p, SyntaxKind::SourceFile);
}

fn statement(p: &mut Parser<'_>) {
    if at_query_start(p) {
        select_stmt(p);
    } else {
        let m = p.start();
        skip_balanced_until(p, |p| p.at(SyntaxKind::Semicolon));
        m.complete(p, SyntaxKind::OpaqueStmt);
    }
}

/// Consumes tokens until `stop` holds outside any parentheses or brackets, or a `)` / `]`
/// closes one opened before the call, or the input ends.
fn skip_balanced_until(p: &mut Parser<'_>, stop: fn(&mut Parser<'_>) -> bool) {
    let mut depth = 0u32;
    while !p.at_end() {
        match p.current() {
            SyntaxKind::LParen | SyntaxKind::LBracket => depth += 1,
            SyntaxKind::RParen | SyntaxKind::RBracket => {
                if depth == 0 {
                    return;
                }
                depth -= 1;
            }
            _ if depth == 0 && stop(p) => return,
            _ => {}
        }
        p.bump();
    }
}

/// Wraps whatever is left before the closing `)` in an `Error` node, then expects the `)`.
fn close_paren(p: &mut Parser<'_>) {
    if !p.at(SyntaxKind::RParen) && !p.at_end() && !p.at(SyntaxKind::Semicolon) {
        p.error("expected )");
        let e = p.start();
        skip_balanced_until(p, |p| p.at(SyntaxKind::Semicolon));
        e.complete(p, SyntaxKind::Error);
    }
    p.expect(SyntaxKind::RParen, ")");
}

/// A possibly qualified name: `a`, `a.b`, `a.b.c`, with `*` allowed last when `star`.
fn name(p: &mut Parser<'_>, star: bool) {
    let m = p.start();
    p.bump();
    while p.at(SyntaxKind::Dot) {
        if matches!(p.nth(1), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
            p.bump();
            p.bump();
        } else if star && p.nth_op(1, "*") {
            p.bump();
            p.bump();
            break;
        } else {
            p.bump();
            p.error("expected a name after .");
            break;
        }
    }
    m.complete(p, SyntaxKind::Name);
}

/// `(a, b, c)`
fn column_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    loop {
        if at_col_id(p) || p.at(SyntaxKind::Ident) {
            p.bump();
        } else {
            p.error("expected a column name");
            break;
        }
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
    close_paren(p);
    m.complete(p, SyntaxKind::ColumnList);
}
