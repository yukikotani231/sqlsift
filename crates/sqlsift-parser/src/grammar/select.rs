//! `SELECT` statements.

use super::{
    at_col_id, close_paren, column_list, expr, is_reserved, is_type_func_name, name, nth_is_col_id,
};
use crate::parser::{CompletedMarker, Parser};
use crate::SyntaxKind;

/// `SELECT ... [FROM] [WHERE] [GROUP BY] [HAVING] [ORDER BY] [LIMIT] [OFFSET]`
pub(crate) fn select_stmt(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    select_clause(p);
    if p.at_kw("from") {
        from_clause(p);
    }
    if p.at_kw("where") {
        let c = p.start();
        p.bump();
        required_expr(p);
        c.complete(p, SyntaxKind::WhereClause);
    }
    if p.at_kw("group") {
        let c = p.start();
        p.bump();
        p.expect_kw("by");
        expr_list(p);
        c.complete(p, SyntaxKind::GroupByClause);
    }
    if p.at_kw("having") {
        let c = p.start();
        p.bump();
        required_expr(p);
        c.complete(p, SyntaxKind::HavingClause);
    }
    if p.at_kw("order") {
        order_by_clause(p);
    }
    // PostgreSQL takes LIMIT and OFFSET in either order.
    for _ in 0..2 {
        if p.at_kw("limit") {
            let c = p.start();
            p.bump();
            if !p.eat_kw("all") {
                required_expr(p);
            }
            c.complete(p, SyntaxKind::LimitClause);
        } else if p.at_kw("offset") {
            let c = p.start();
            p.bump();
            required_expr(p);
            if !p.eat_kw("rows") {
                p.eat_kw("row");
            }
            c.complete(p, SyntaxKind::OffsetClause);
        }
    }
    m.complete(p, SyntaxKind::SelectStmt)
}

fn required_expr(p: &mut Parser<'_>) {
    if expr(p).is_none() {
        p.error("expected an expression");
    }
}

/// `a, b, c` (at least one)
fn expr_list(p: &mut Parser<'_>) {
    let m = p.start();
    loop {
        required_expr(p);
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
    m.complete(p, SyntaxKind::ExprList);
}

fn select_clause(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(); // SELECT
    if p.at_kw("distinct") {
        p.bump();
        if p.at_kw("on") {
            p.bump();
            if p.at(SyntaxKind::LParen) {
                p.bump();
                expr_list(p);
                close_paren(p);
            } else {
                p.error("expected (");
            }
        }
    } else {
        p.eat_kw("all");
    }
    target_list(p);
    m.complete(p, SyntaxKind::SelectClause);
}

/// Words after which a target list is over (the next clause, or the end of the query).
fn at_target_list_end(p: &mut Parser<'_>) -> bool {
    p.at_end()
        || matches!(p.current(), SyntaxKind::Semicolon | SyntaxKind::RParen)
        || p.at_any_kw(&[
            "from",
            "where",
            "group",
            "having",
            "order",
            "limit",
            "offset",
            "union",
            "intersect",
            "except",
            "window",
            "fetch",
            "for",
            "into",
        ])
}

fn target_list(p: &mut Parser<'_>) {
    let m = p.start();
    // PostgreSQL allows an empty target list (`SELECT FROM t`).
    if !at_target_list_end(p) {
        loop {
            target_item(p);
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
    }
    m.complete(p, SyntaxKind::TargetList);
}

fn target_item(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at_op("*") {
        let s = p.start();
        p.bump();
        s.complete(p, SyntaxKind::Star);
    } else {
        if expr(p).is_none() {
            p.error("expected an expression");
        }
        if p.at_kw("as") {
            let a = p.start();
            p.bump();
            if matches!(p.current(), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
                p.bump();
            } else {
                p.error("expected an alias");
            }
            a.complete(p, SyntaxKind::Alias);
        } else if at_bare_label(p) {
            let a = p.start();
            p.bump();
            a.complete(p, SyntaxKind::Alias);
        }
    }
    m.complete(p, SyntaxKind::TargetItem);
}

/// A column alias without `AS`.
fn at_bare_label(p: &mut Parser<'_>) -> bool {
    match p.current() {
        SyntaxKind::QuotedIdent => true,
        SyntaxKind::Ident => {
            let word = p.nth_text(0);
            !is_reserved(word) && !is_type_func_name(word)
        }
        _ => false,
    }
}

fn from_clause(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(); // FROM
    loop {
        from_item(p);
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
    m.complete(p, SyntaxKind::FromClause);
}

/// A FROM item with its joins.
fn from_item(p: &mut Parser<'_>) {
    let Some(mut lhs) = from_primary(p) else {
        return;
    };
    loop {
        let natural = p.at_kw("natural");
        let n = usize::from(natural);
        let (kw_len, needs_condition) = if p.nth_kw(n, "join") {
            (1, true)
        } else if p.nth_kw(n, "inner") && p.nth_kw(n + 1, "join") {
            (2, true)
        } else if p.nth_kw(n, "cross") && p.nth_kw(n + 1, "join") {
            (2, false)
        } else if ["left", "right", "full"].iter().any(|kw| p.nth_kw(n, kw)) {
            if p.nth_kw(n + 1, "outer") && p.nth_kw(n + 2, "join") {
                (3, true)
            } else if p.nth_kw(n + 1, "join") {
                (2, true)
            } else {
                break;
            }
        } else {
            break;
        };
        let m = lhs.precede(p);
        for _ in 0..n + kw_len {
            p.bump();
        }
        if from_primary(p).is_none() {
            m.complete(p, SyntaxKind::JoinExpr);
            break;
        }
        if needs_condition && !natural {
            join_condition(p);
        }
        lhs = m.complete(p, SyntaxKind::JoinExpr);
    }
}

fn join_condition(p: &mut Parser<'_>) {
    let m = p.start();
    if p.eat_kw("on") {
        if expr(p).is_none() {
            p.error("expected an expression");
        }
    } else if p.eat_kw("using") {
        if p.at(SyntaxKind::LParen) {
            column_list(p);
        } else {
            p.error("expected (");
        }
    } else {
        p.error("expected ON or USING");
    }
    m.complete(p, SyntaxKind::JoinCondition);
}

fn from_primary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    if p.at(SyntaxKind::LParen) {
        let m = p.start();
        p.bump();
        let kind = if p.at_kw("select") {
            super::select_stmt(p);
            SyntaxKind::SubqueryRef
        } else {
            from_item(p);
            SyntaxKind::ParenFromItem
        };
        close_paren(p);
        table_alias(p);
        return Some(m.complete(p, kind));
    }
    if at_col_id(p) {
        let m = p.start();
        name(p, false);
        if p.at(SyntaxKind::LParen) {
            // Table functions come later.
            p.error("table functions are not supported yet");
        }
        table_alias(p);
        return Some(m.complete(p, SyntaxKind::TableRef));
    }
    p.error("expected a table name or (");
    None
}

/// `[AS] alias [(col, ...)]`
fn table_alias(p: &mut Parser<'_>) {
    let has_as = p.at_kw("as");
    let n = usize::from(has_as);
    if !(nth_is_col_id(p, n) || (has_as && p.nth(n) == SyntaxKind::Ident)) {
        if has_as {
            p.err_and_bump("expected an alias");
        }
        return;
    }
    let m = p.start();
    for _ in 0..=n {
        p.bump();
    }
    if p.at(SyntaxKind::LParen) {
        column_list(p);
    }
    m.complete(p, SyntaxKind::Alias);
}

fn order_by_clause(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(); // ORDER
    p.expect_kw("by");
    loop {
        let s = p.start();
        required_expr(p);
        if !p.eat_kw("asc") && !p.eat_kw("desc") && p.at_kw("using") {
            p.bump();
            if !p.eat(SyntaxKind::Operator) {
                p.error("expected an operator");
            }
        }
        if p.at_kw("nulls") && (p.nth_kw(1, "first") || p.nth_kw(1, "last")) {
            p.bump();
            p.bump();
        }
        s.complete(p, SyntaxKind::SortItem);
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
    m.complete(p, SyntaxKind::OrderByClause);
}
