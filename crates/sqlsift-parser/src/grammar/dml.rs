//! `INSERT`, `UPDATE` and `DELETE`.

use super::select::{from_item, required_expr, select_stmt, table_alias, target_list};
use super::{at_col_id, at_query_start, close_paren, column_list, name};
use crate::parser::{CompletedMarker, Parser};
use crate::SyntaxKind;

pub(crate) fn at_dml_start(p: &mut Parser<'_>) -> bool {
    (p.at_kw("insert") && p.nth_kw(1, "into"))
        || p.at_kw("update")
        || (p.at_kw("delete") && p.nth_kw(1, "from"))
}

pub(crate) fn dml_stmt(p: &mut Parser<'_>) -> CompletedMarker {
    if p.at_kw("insert") {
        insert(p)
    } else if p.at_kw("update") {
        update(p)
    } else {
        delete(p)
    }
}

/// The target table: `[ONLY] name [AS alias]` (no column aliases).
fn target_table(p: &mut Parser<'_>) {
    p.eat_kw("only");
    if at_col_id(p) {
        name(p, false);
    } else {
        p.error("expected a table name");
    }
}

/// `INSERT INTO t [AS a] [(cols)] [OVERRIDING ...] {DEFAULT VALUES | query}
/// [ON CONFLICT ...] [RETURNING ...]`
fn insert(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    p.bump();
    target_table(p);
    // `INSERT INTO t AS a`: PostgreSQL requires AS here, and takes no column aliases.
    if p.at_kw("as") {
        let a = p.start();
        p.bump();
        if at_col_id(p) {
            p.bump();
        } else {
            p.error("expected an alias");
        }
        a.complete(p, SyntaxKind::Alias);
    }
    if p.at(SyntaxKind::LParen) && !at_query_start(p) {
        column_list(p);
    }
    if p.at_kw("overriding") {
        let c = p.start();
        p.bump();
        if !p.eat_kw("system") {
            p.expect_kw("user");
        }
        p.expect_kw("value");
        c.complete(p, SyntaxKind::OverridingClause);
    }
    if p.at_kw("default") && p.nth_kw(1, "values") {
        p.bump();
        p.bump();
    } else if at_query_start(p) {
        select_stmt(p);
    } else {
        p.error("expected VALUES, a query or DEFAULT VALUES");
    }
    if p.at_kw("on") && p.nth_kw(1, "conflict") {
        on_conflict(p);
    }
    returning(p);
    m.complete(p, SyntaxKind::InsertStmt)
}

fn on_conflict(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    p.bump();
    if p.at(SyntaxKind::LParen) || (p.at_kw("on") && p.nth_kw(1, "constraint")) {
        let t = p.start();
        if p.eat_kw("on") {
            p.bump();
            if at_col_id(p) {
                name(p, false);
            } else {
                p.error("expected a constraint name");
            }
        } else {
            // Index expressions: columns, or expressions in parentheses.
            p.bump();
            loop {
                required_expr(p);
                if !p.eat(SyntaxKind::Comma) {
                    break;
                }
            }
            close_paren(p);
            if p.at_kw("where") {
                let w = p.start();
                p.bump();
                required_expr(p);
                w.complete(p, SyntaxKind::WhereClause);
            }
        }
        t.complete(p, SyntaxKind::ConflictTarget);
    }
    let a = p.start();
    p.expect_kw("do");
    if !p.eat_kw("nothing") {
        p.expect_kw("update");
        set_clause(p);
        where_clause(p);
    }
    a.complete(p, SyntaxKind::ConflictAction);
    m.complete(p, SyntaxKind::OnConflictClause);
}

/// `UPDATE [ONLY] t [[AS] a] SET ... [FROM ...] [WHERE ...] [RETURNING ...]`
fn update(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    target_table(p);
    if !p.at_kw("set") {
        table_alias(p);
    }
    set_clause(p);
    if p.at_kw("from") {
        let c = p.start();
        p.bump();
        loop {
            from_item(p);
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
        c.complete(p, SyntaxKind::FromClause);
    }
    where_clause(p);
    returning(p);
    m.complete(p, SyntaxKind::UpdateStmt)
}

/// `DELETE FROM [ONLY] t [[AS] a] [USING ...] [WHERE ...] [RETURNING ...]`
fn delete(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    p.bump();
    target_table(p);
    if !p.at_any_kw(&["using", "where", "returning"]) {
        table_alias(p);
    }
    if p.at_kw("using") {
        let c = p.start();
        p.bump();
        loop {
            from_item(p);
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
        c.complete(p, SyntaxKind::UsingClause);
    }
    where_clause(p);
    returning(p);
    m.complete(p, SyntaxKind::DeleteStmt)
}

fn where_clause(p: &mut Parser<'_>) {
    if p.at_kw("where") {
        let c = p.start();
        p.bump();
        if p.at_kw("current") && p.nth_kw(1, "of") {
            // `WHERE CURRENT OF cursor`
            p.bump();
            p.bump();
            if !p.eat(SyntaxKind::Ident) {
                p.error("expected a cursor name");
            }
        } else {
            required_expr(p);
        }
        c.complete(p, SyntaxKind::WhereClause);
    }
}

fn returning(p: &mut Parser<'_>) {
    if p.at_kw("returning") {
        let c = p.start();
        p.bump();
        target_list(p);
        c.complete(p, SyntaxKind::ReturningClause);
    }
}

/// `SET a = 1, b.f = 2, (c, d) = (3, 4), (e) = (SELECT ...)`
fn set_clause(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.expect_kw("set") {
        m.complete(p, SyntaxKind::SetClause);
        return;
    }
    loop {
        let a = p.start();
        if p.at(SyntaxKind::LParen) {
            column_list(p);
        } else if matches!(p.current(), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
            name(p, false);
        } else {
            p.error("expected a column name");
        }
        if p.at_op("=") {
            p.bump();
            required_expr(p);
        } else {
            p.error("expected =");
        }
        a.complete(p, SyntaxKind::Assignment);
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
    m.complete(p, SyntaxKind::SetClause);
}
