//! Expressions: a Pratt parser with PostgreSQL's operator precedence (the table in the
//! "Operator Precedence" section of the manual, as sqlparser's PostgreSQL dialect has it).

use super::{close_paren, is_reserved, name, select_stmt, types::type_name};
use crate::parser::{CompletedMarker, Parser};
use crate::SyntaxKind;

const OR: u8 = 10;
const AND: u8 = 20;
const NOT: u8 = 30;
const IS: u8 = 40;
const CMP: u8 = 50;
const LIKE: u8 = 60;
const OTHER_OP: u8 = 70;
const PLUS: u8 = 80;
const MUL: u8 = 90;
const CARET: u8 = 100;
const SUBSCRIPT: u8 = 130;
const CAST: u8 = 140;

/// Reserved keywords that are values or expression forms rather than names.
const VALUE_KEYWORDS: &[&str] = &[
    "current_catalog",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "localtime",
    "localtimestamp",
    "session_user",
    "system_user",
    "user",
];

#[derive(Clone, Copy)]
enum Infix {
    Binary,
    /// `IS ...`, `ISNULL`, `NOTNULL`
    Is,
    /// `[NOT] IN`, `[NOT] BETWEEN`, `[NOT] LIKE / ILIKE / SIMILAR TO`
    Predicate,
    Cast,
    Subscript,
}

/// An expression. Returns `None`, consuming nothing, if there is none: the caller reports it.
pub(crate) fn expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    expr_bp(p, 0)
}

fn expr_bp(p: &mut Parser<'_>, min: u8) -> Option<CompletedMarker> {
    let mut lhs = prefix(p)?;
    while let Some((prec, infix)) = infix_op(p) {
        if prec <= min {
            break;
        }
        let m = lhs.precede(p);
        let kind = match infix {
            Infix::Binary => {
                p.bump();
                operand(p, prec);
                SyntaxKind::BinaryExpr
            }
            Infix::Is => is_tail(p),
            Infix::Predicate => predicate_tail(p),
            Infix::Cast => {
                p.bump();
                type_name(p);
                SyntaxKind::TypeCastExpr
            }
            Infix::Subscript => {
                subscript_tail(p);
                SyntaxKind::SubscriptExpr
            }
        };
        lhs = m.complete(p, kind);
    }
    Some(lhs)
}

/// The right-hand side of an operator; records an error if missing.
fn operand(p: &mut Parser<'_>, prec: u8) {
    if expr_bp(p, prec).is_none() {
        p.error("expected an expression");
    }
}

fn infix_op(p: &mut Parser<'_>) -> Option<(u8, Infix)> {
    match p.current() {
        SyntaxKind::DoubleColon => return Some((CAST, Infix::Cast)),
        SyntaxKind::LBracket => return Some((SUBSCRIPT, Infix::Subscript)),
        SyntaxKind::Operator => {
            let prec = match p.nth_text(0) {
                "=" | "<" | ">" | "<=" | ">=" | "<>" | "!=" => CMP,
                "+" | "-" => PLUS,
                "*" | "/" | "%" => MUL,
                "^" => CARET,
                ":=" => return None,
                _ => OTHER_OP,
            };
            return Some((prec, Infix::Binary));
        }
        SyntaxKind::Ident => {}
        _ => return None,
    }
    if p.at_kw("or") {
        Some((OR, Infix::Binary))
    } else if p.at_kw("and") {
        Some((AND, Infix::Binary))
    } else if p.at_any_kw(&["is", "isnull", "notnull"]) {
        Some((IS, Infix::Is))
    } else if p.at_any_kw(&["in", "between", "like", "ilike", "similar"])
        || (p.at_kw("not")
            && ["in", "between", "like", "ilike", "similar"]
                .iter()
                .any(|kw| p.nth_kw(1, kw)))
    {
        Some((LIKE, Infix::Predicate))
    } else {
        None
    }
}

/// After the left operand: `IS [NOT] NULL | TRUE | FALSE | UNKNOWN | DISTINCT FROM x`,
/// `ISNULL`, `NOTNULL`.
fn is_tail(p: &mut Parser<'_>) -> SyntaxKind {
    if p.eat_kw("isnull") || p.eat_kw("notnull") {
        return SyntaxKind::IsExpr;
    }
    p.bump(); // IS
    p.eat_kw("not");
    if p.at_any_kw(&["null", "true", "false", "unknown"]) {
        p.bump();
    } else if p.at_kw("distinct") {
        p.bump();
        p.expect_kw("from");
        operand(p, IS);
    } else {
        p.error("expected NULL, TRUE, FALSE, UNKNOWN or DISTINCT FROM");
    }
    SyntaxKind::IsExpr
}

/// After the left operand: `[NOT] IN (...)`, `[NOT] BETWEEN [SYMMETRIC] a AND b`,
/// `[NOT] LIKE / ILIKE x [ESCAPE y]`, `[NOT] SIMILAR TO x [ESCAPE y]`.
fn predicate_tail(p: &mut Parser<'_>) -> SyntaxKind {
    p.eat_kw("not");
    if p.eat_kw("in") {
        if p.at(SyntaxKind::LParen) {
            let m = p.start();
            p.bump();
            let kind = if p.at_kw("select") {
                select_stmt(p);
                SyntaxKind::SubqueryExpr
            } else {
                list_items(p);
                SyntaxKind::ExprList
            };
            close_paren(p);
            m.complete(p, kind);
        } else {
            p.error("expected (");
        }
        return SyntaxKind::InExpr;
    }
    if p.eat_kw("between") {
        if p.at_kw("symmetric") || p.at_kw("asymmetric") {
            p.bump();
        }
        operand(p, LIKE);
        p.expect_kw("and");
        operand(p, LIKE);
        return SyntaxKind::BetweenExpr;
    }
    if p.eat_kw("similar") {
        p.expect_kw("to");
    } else {
        p.bump(); // LIKE / ILIKE
    }
    operand(p, LIKE);
    if p.eat_kw("escape") {
        operand(p, LIKE);
    }
    SyntaxKind::LikeExpr
}

fn subscript_tail(p: &mut Parser<'_>) {
    p.bump(); // [
    if !p.at(SyntaxKind::Colon) {
        operand(p, 0);
    }
    if p.eat(SyntaxKind::Colon) && !p.at(SyntaxKind::RBracket) {
        operand(p, 0);
    }
    p.expect(SyntaxKind::RBracket, "]");
}

/// `a, b, c` inside parentheses (at least one), without a node.
fn list_items(p: &mut Parser<'_>) {
    loop {
        operand(p, 0);
        if !p.eat(SyntaxKind::Comma) {
            break;
        }
    }
}

fn prefix(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    if p.at_kw("not") {
        let m = p.start();
        p.bump();
        operand(p, NOT);
        return Some(m.complete(p, SyntaxKind::PrefixExpr));
    }
    if p.at_op("-") || p.at_op("+") {
        let m = p.start();
        p.bump();
        operand(p, MUL);
        return Some(m.complete(p, SyntaxKind::PrefixExpr));
    }
    atom(p)
}

fn atom(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    match p.current() {
        SyntaxKind::Number
        | SyntaxKind::String
        | SyntaxKind::EscapeString
        | SyntaxKind::NationalString
        | SyntaxKind::BitString
        | SyntaxKind::HexString
        | SyntaxKind::UnicodeString
        | SyntaxKind::DollarString => Some(single(p, SyntaxKind::Literal)),
        SyntaxKind::Param => Some(single(p, SyntaxKind::ParamRef)),
        SyntaxKind::LParen => Some(paren(p)),
        SyntaxKind::QuotedIdent => Some(name_or_call(p)),
        SyntaxKind::Ident => {
            if p.at_any_kw(&["true", "false", "null"]) {
                Some(single(p, SyntaxKind::Literal))
            } else if p.at_kw("case") {
                Some(case(p))
            } else if p.at_kw("cast") && p.nth(1) == SyntaxKind::LParen {
                Some(cast(p))
            } else if p.at_kw("exists") && p.nth(1) == SyntaxKind::LParen {
                Some(exists(p))
            } else if VALUE_KEYWORDS
                .iter()
                .any(|kw| p.nth_text(0).eq_ignore_ascii_case(kw))
            {
                let m = p.start();
                name(p, false);
                Some(m.complete(p, SyntaxKind::ColumnRef))
            } else if is_reserved(p.nth_text(0)) {
                None
            } else {
                Some(name_or_call(p))
            }
        }
        _ => None,
    }
}

fn single(p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
    let m = p.start();
    p.bump();
    m.complete(p, kind)
}

/// `(x)`, `(a, b)` or `(SELECT ...)`
fn paren(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    if p.at_kw("select") {
        select_stmt(p);
        close_paren(p);
        return m.complete(p, SyntaxKind::SubqueryExpr);
    }
    operand(p, 0);
    let kind = if p.at(SyntaxKind::Comma) {
        while p.eat(SyntaxKind::Comma) {
            operand(p, 0);
        }
        SyntaxKind::RowExpr
    } else {
        SyntaxKind::ParenExpr
    };
    close_paren(p);
    m.complete(p, kind)
}

/// A column reference (`a`, `t.a`, `t.*`) or a function call (`f(x)`, `s.f(x)`).
fn name_or_call(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    name(p, true);
    if !p.at(SyntaxKind::LParen) {
        return m.complete(p, SyntaxKind::ColumnRef);
    }
    let args = p.start();
    p.bump();
    if p.at_op("*") {
        let s = p.start();
        p.bump();
        s.complete(p, SyntaxKind::Star);
    } else if !p.at(SyntaxKind::RParen) {
        if p.at_kw("distinct") || p.at_kw("all") {
            p.bump();
        }
        list_items(p);
    }
    close_paren(p);
    args.complete(p, SyntaxKind::ArgList);
    m.complete(p, SyntaxKind::FuncCall)
}

/// `CASE [x] WHEN a THEN b ... [ELSE c] END`
fn case(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    if !p.at_kw("when") {
        operand(p, 0);
    }
    if !p.at_kw("when") {
        p.error("expected WHEN");
    }
    while p.at_kw("when") {
        let w = p.start();
        p.bump();
        operand(p, 0);
        p.expect_kw("then");
        operand(p, 0);
        w.complete(p, SyntaxKind::WhenClause);
    }
    if p.at_kw("else") {
        let e = p.start();
        p.bump();
        operand(p, 0);
        e.complete(p, SyntaxKind::ElseClause);
    }
    p.expect_kw("end");
    m.complete(p, SyntaxKind::CaseExpr)
}

/// `CAST(x AS type)`
fn cast(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    p.bump();
    operand(p, 0);
    p.expect_kw("as");
    type_name(p);
    close_paren(p);
    m.complete(p, SyntaxKind::CastExpr)
}

/// `EXISTS (SELECT ...)`
fn exists(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    p.bump();
    if p.at_kw("select") {
        select_stmt(p);
    } else {
        p.error("expected SELECT");
    }
    close_paren(p);
    m.complete(p, SyntaxKind::ExistsExpr)
}
