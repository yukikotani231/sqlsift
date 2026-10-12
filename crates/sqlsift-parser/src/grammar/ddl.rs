//! Schema statements: `CREATE TABLE` (more to come).

use super::expr::b_expr;
use super::select::{required_expr, select_stmt};
use super::types::type_name;
use super::{at_col_id, at_query_start, close_paren, column_list, name, skip_balanced_until};
use crate::parser::{CompletedMarker, Parser};
use crate::SyntaxKind;

pub(crate) fn at_ddl_start(p: &mut Parser<'_>) -> bool {
    p.at_kw("create") && nth_is_create_table(p, 1)
}

/// `[GLOBAL | LOCAL] {TEMP | TEMPORARY} | UNLOGGED` and then `TABLE`, from the n-th token.
fn nth_is_create_table(p: &mut Parser<'_>, mut n: usize) -> bool {
    if p.nth_kw(n, "global") || p.nth_kw(n, "local") {
        n += 1;
    }
    if p.nth_kw(n, "temp") || p.nth_kw(n, "temporary") || p.nth_kw(n, "unlogged") {
        n += 1;
    }
    p.nth_kw(n, "table")
}

pub(crate) fn ddl_stmt(p: &mut Parser<'_>) -> CompletedMarker {
    create_table(p)
}

/// `CREATE [TEMP] TABLE [IF NOT EXISTS] name ( elements ) [options]`, or
/// `CREATE TABLE name [(columns)] AS query [WITH [NO] DATA]`.
fn create_table(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(); // CREATE
    if p.at_kw("global") || p.at_kw("local") {
        p.bump();
    }
    if p.at_any_kw(&["temp", "temporary", "unlogged"]) {
        p.bump();
    }
    p.bump(); // TABLE
    if p.at_kw("if") && p.nth_kw(1, "not") && p.nth_kw(2, "exists") {
        p.bump();
        p.bump();
        p.bump();
    }
    if at_col_id(p) {
        name(p, false);
    } else {
        p.error("expected a table name");
    }
    if p.at(SyntaxKind::LParen) && after_parens_is_as(p) {
        column_list(p);
    } else if p.at(SyntaxKind::LParen) {
        table_elements(p);
    }
    while table_option(p) {}
    if p.at_kw("as") {
        p.bump();
        if at_query_start(p) {
            select_stmt(p);
        } else {
            p.error("expected a query");
        }
        if p.at_kw("with") && (p.nth_kw(1, "data") || (p.nth_kw(1, "no") && p.nth_kw(2, "data"))) {
            let o = p.start();
            p.bump();
            p.eat_kw("no");
            p.bump();
            o.complete(p, SyntaxKind::TableOption);
        }
    }
    m.complete(p, SyntaxKind::CreateTableStmt)
}

/// One table option after the elements, or `false` if there is none:
/// `INHERITS (t, ...)`, `PARTITION BY strategy (...)`, `USING method`, `WITH (...)`,
/// `WITHOUT OIDS`, `ON COMMIT ...`, `TABLESPACE name`.
fn table_option(p: &mut Parser<'_>) -> bool {
    let m = p.start();
    if p.at_kw("inherits") && p.nth(1) == SyntaxKind::LParen {
        p.bump();
        p.bump();
        loop {
            if at_col_id(p) {
                name(p, false);
            } else {
                p.error("expected a table name");
                break;
            }
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
        close_paren(p);
    } else if p.at_kw("partition") && p.nth_kw(1, "by") {
        p.bump();
        p.bump();
        // `RANGE (a, b)` reads as a call, which is how sqlparser takes it too.
        required_expr(p);
    } else if p.at_kw("using") {
        p.bump();
        constraint_name(p);
    } else if p.at_kw("with") && p.nth(1) == SyntaxKind::LParen {
        p.bump();
        p.bump();
        skip_balanced_until(p, |_| false);
        p.expect(SyntaxKind::RParen, ")");
    } else if p.at_kw("without") && p.nth_kw(1, "oids") {
        p.bump();
        p.bump();
    } else if p.at_kw("on") && p.nth_kw(1, "commit") {
        p.bump();
        p.bump();
        if p.eat_kw("delete") || p.eat_kw("preserve") {
            p.expect_kw("rows");
        } else {
            p.expect_kw("drop");
        }
    } else if p.at_kw("tablespace") {
        p.bump();
        constraint_name(p);
    } else {
        m.abandon(p);
        return false;
    }
    m.complete(p, SyntaxKind::TableOption);
    true
}

/// Whether the parenthesized list at the current token is followed by `AS` (a `CREATE
/// TABLE ... AS` column list rather than table elements).
fn after_parens_is_as(p: &mut Parser<'_>) -> bool {
    let mut depth = 0u32;
    let mut n = 0;
    while !p.nth_at_end(n) {
        match p.nth(n) {
            SyntaxKind::LParen => depth += 1,
            SyntaxKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return p.nth_kw(n + 1, "as");
                }
            }
            SyntaxKind::Semicolon => return false,
            _ => {}
        }
        n += 1;
    }
    false
}

/// `( element, ... )`: column definitions, table constraints and `LIKE` clauses.
fn table_elements(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    if !p.at(SyntaxKind::RParen) {
        loop {
            table_element(p);
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
    }
    close_paren(p);
    m.complete(p, SyntaxKind::TableElements);
}

fn table_element(p: &mut Parser<'_>) {
    if p.at_kw("like") {
        let m = p.start();
        p.bump();
        if at_col_id(p) {
            name(p, false);
        } else {
            p.error("expected a table name");
        }
        while p.at_kw("including") || p.at_kw("excluding") {
            p.bump();
            if matches!(p.current(), SyntaxKind::Ident) {
                p.bump();
            } else {
                p.error("expected a LIKE option");
            }
        }
        m.complete(p, SyntaxKind::TableLikeClause);
    } else if p.at_any_kw(&[
        "constraint",
        "check",
        "unique",
        "primary",
        "exclude",
        "foreign",
    ]) {
        table_constraint(p);
    } else if at_col_id(p) || p.at(SyntaxKind::Ident) {
        column_def(p);
    } else {
        p.error("expected a column definition or a constraint");
        let e = p.start();
        skip_balanced_until(p, |p| {
            p.at(SyntaxKind::Comma) || p.at(SyntaxKind::Semicolon)
        });
        e.complete(p, SyntaxKind::Error);
    }
}

/// `name type [COLLATE c] [constraint ...]`
fn column_def(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    type_name(p);
    loop {
        if p.at_kw("collate") {
            let c = p.start();
            p.bump();
            if matches!(p.current(), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
                name(p, false);
            } else {
                p.error("expected a collation name");
            }
            c.complete(p, SyntaxKind::ColumnConstraint);
        } else if at_column_constraint(p) {
            column_constraint(p);
        } else {
            break;
        }
    }
    m.complete(p, SyntaxKind::ColumnDef);
}

fn at_column_constraint(p: &mut Parser<'_>) -> bool {
    p.at_any_kw(&[
        "constraint",
        "null",
        "check",
        "default",
        "generated",
        "unique",
        "primary",
        "references",
    ]) || (p.at_kw("not") && p.nth_kw(1, "null"))
        || at_constraint_attribute(p)
}

/// One column constraint (`ColConstraint` in `gram.y`).
fn column_constraint(p: &mut Parser<'_>) {
    let m = p.start();
    if p.eat_kw("constraint") {
        constraint_name(p);
    }
    if p.at_kw("not") && p.nth_kw(1, "null") {
        p.bump();
        p.bump();
    } else if p.at_kw("null") {
        p.bump();
    } else if p.at_kw("check") {
        check_body(p);
    } else if p.at_kw("default") {
        p.bump();
        if b_expr(p).is_none() {
            p.error("expected an expression");
        }
    } else if p.at_kw("generated") {
        generated(p);
    } else if p.at_kw("unique") {
        p.bump();
        nulls_distinct(p);
        index_params(p);
    } else if p.at_kw("primary") {
        p.bump();
        p.expect_kw("key");
        index_params(p);
    } else if p.at_kw("references") {
        references(p);
    } else if !at_constraint_attribute(p) {
        p.error("expected a constraint");
    }
    constraint_attributes(p);
    m.complete(p, SyntaxKind::ColumnConstraint);
}

fn constraint_name(p: &mut Parser<'_>) {
    if at_col_id(p) || p.at(SyntaxKind::Ident) {
        let n = p.start();
        p.bump();
        n.complete(p, SyntaxKind::Name);
    } else {
        p.error("expected a constraint name");
    }
}

/// `CHECK (expr) [NO INHERIT]`
fn check_body(p: &mut Parser<'_>) {
    p.bump();
    if p.eat(SyntaxKind::LParen) {
        required_expr(p);
        close_paren(p);
    } else {
        p.error("expected (");
    }
    if p.at_kw("no") && p.nth_kw(1, "inherit") {
        p.bump();
        p.bump();
    }
}

/// `GENERATED ALWAYS AS (expr) STORED` or
/// `GENERATED {ALWAYS | BY DEFAULT} AS IDENTITY [(sequence options)]`
fn generated(p: &mut Parser<'_>) {
    p.bump();
    if p.eat_kw("by") {
        p.expect_kw("default");
    } else {
        p.expect_kw("always");
    }
    p.expect_kw("as");
    if p.eat_kw("identity") {
        if p.at(SyntaxKind::LParen) {
            let o = p.start();
            p.bump();
            skip_balanced_until(p, |_| false);
            p.expect(SyntaxKind::RParen, ")");
            o.complete(p, SyntaxKind::SequenceOptions);
        }
    } else if p.eat(SyntaxKind::LParen) {
        required_expr(p);
        close_paren(p);
        if !p.eat_kw("stored") {
            p.eat_kw("virtual");
        }
    } else {
        p.error("expected IDENTITY or (");
    }
}

/// `[NULLS [NOT] DISTINCT]`
fn nulls_distinct(p: &mut Parser<'_>) {
    if p.at_kw("nulls")
        && (p.nth_kw(1, "distinct") || (p.nth_kw(1, "not") && p.nth_kw(2, "distinct")))
    {
        p.bump();
        p.eat_kw("not");
        p.bump();
    }
}

/// `[INCLUDE (cols)] [WITH (params)] [USING INDEX TABLESPACE name]` after a unique or
/// primary key constraint.
fn index_params(p: &mut Parser<'_>) {
    if p.at_kw("include") && p.nth(1) == SyntaxKind::LParen {
        let i = p.start();
        p.bump();
        column_list(p);
        i.complete(p, SyntaxKind::IndexParams);
    }
    if p.at_kw("with") && p.nth(1) == SyntaxKind::LParen {
        let i = p.start();
        p.bump();
        p.bump();
        skip_balanced_until(p, |_| false);
        p.expect(SyntaxKind::RParen, ")");
        i.complete(p, SyntaxKind::IndexParams);
    }
    if p.at_kw("using") && p.nth_kw(1, "index") && p.nth_kw(2, "tablespace") {
        let i = p.start();
        p.bump();
        p.bump();
        p.bump();
        constraint_name(p);
        i.complete(p, SyntaxKind::IndexParams);
    }
}

/// `REFERENCES table [(cols)] [MATCH FULL | PARTIAL | SIMPLE]
/// [ON DELETE action] [ON UPDATE action]`
fn references(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump();
    if at_col_id(p) {
        name(p, false);
    } else {
        p.error("expected a table name");
    }
    if p.at(SyntaxKind::LParen) {
        column_list(p);
    }
    if p.at_kw("match") {
        let k = p.start();
        p.bump();
        if p.at_any_kw(&["full", "partial", "simple"]) {
            p.bump();
        } else {
            p.error("expected FULL, PARTIAL or SIMPLE");
        }
        k.complete(p, SyntaxKind::MatchClause);
    }
    while p.at_kw("on") && (p.nth_kw(1, "delete") || p.nth_kw(1, "update")) {
        let a = p.start();
        p.bump();
        p.bump();
        if p.at_kw("no") && p.nth_kw(1, "action") {
            p.bump();
            p.bump();
        } else if p.at_kw("restrict") || p.at_kw("cascade") {
            p.bump();
        } else if p.at_kw("set") && (p.nth_kw(1, "null") || p.nth_kw(1, "default")) {
            p.bump();
            p.bump();
            if p.at(SyntaxKind::LParen) {
                column_list(p);
            }
        } else {
            p.error("expected a referential action");
        }
        a.complete(p, SyntaxKind::ReferentialAction);
    }
    m.complete(p, SyntaxKind::ReferencesClause);
}

fn at_constraint_attribute(p: &mut Parser<'_>) -> bool {
    p.at_kw("deferrable")
        || p.at_kw("initially")
        || (p.at_kw("not") && (p.nth_kw(1, "deferrable") || p.nth_kw(1, "valid")))
        || (p.at_kw("no") && p.nth_kw(1, "inherit"))
}

/// `[NOT] DEFERRABLE`, `INITIALLY {DEFERRED | IMMEDIATE}`, `NOT VALID`, `NO INHERIT`.
fn constraint_attributes(p: &mut Parser<'_>) {
    while at_constraint_attribute(p) {
        let a = p.start();
        if p.eat_kw("initially") {
            if !p.eat_kw("deferred") {
                p.expect_kw("immediate");
            }
        } else {
            p.eat_kw("not");
            p.eat_kw("no");
            p.bump();
        }
        a.complete(p, SyntaxKind::ConstraintAttribute);
    }
}

/// `[CONSTRAINT name] {CHECK (expr) | UNIQUE (cols) | PRIMARY KEY (cols) |
/// FOREIGN KEY (cols) REFERENCES ... | EXCLUDE ...} [attributes]`
fn table_constraint(p: &mut Parser<'_>) {
    let m = p.start();
    if p.eat_kw("constraint") {
        constraint_name(p);
    }
    if p.at_kw("check") {
        check_body(p);
    } else if p.at_kw("unique") || p.at_kw("primary") {
        if p.eat_kw("primary") {
            p.expect_kw("key");
        } else {
            p.bump();
            nulls_distinct(p);
        }
        if p.at(SyntaxKind::LParen) {
            column_list(p);
            index_params(p);
        } else if p.at_kw("using") && p.nth_kw(1, "index") {
            p.bump();
            p.bump();
            constraint_name(p);
        } else {
            p.error("expected (");
        }
    } else if p.at_kw("foreign") {
        p.bump();
        p.expect_kw("key");
        if p.at(SyntaxKind::LParen) {
            column_list(p);
        } else {
            p.error("expected (");
        }
        if p.at_kw("references") {
            references(p);
        } else {
            p.error("expected REFERENCES");
        }
    } else if p.at_kw("exclude") {
        // EXCLUDE [USING method] (elements) [params] [WHERE (pred)]: kept, not parsed.
        let e = p.start();
        p.bump();
        skip_balanced_until(p, |p| {
            p.at(SyntaxKind::Comma) || p.at(SyntaxKind::Semicolon) || at_constraint_attribute(p)
        });
        e.complete(p, SyntaxKind::ExcludeBody);
    } else {
        p.error("expected CHECK, UNIQUE, PRIMARY KEY, FOREIGN KEY or EXCLUDE");
    }
    constraint_attributes(p);
    m.complete(p, SyntaxKind::TableConstraint);
}
