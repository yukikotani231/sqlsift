//! Type names (`Typename` in `gram.y`), for casts.

use crate::parser::Parser;
use crate::SyntaxKind;

/// A type name: `int`, `varchar(20)`, `double precision`, `timestamp with time zone`,
/// `public.my_type`, `text[]`.
pub(crate) fn type_name(p: &mut Parser<'_>) {
    let m = p.start();
    if !matches!(p.current(), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
        p.error("expected a type name");
        m.complete(p, SyntaxKind::TypeName);
        return;
    }
    let first = p.nth_text(0).to_ascii_lowercase();
    p.bump();
    match first.as_str() {
        "double" => {
            p.expect_kw("precision");
        }
        "character" | "char" | "bit" | "national" => {
            if first == "national" && !p.eat_kw("character") {
                p.eat_kw("char");
            }
            p.eat_kw("varying");
        }
        _ => {
            while p.at(SyntaxKind::Dot)
                && matches!(p.nth(1), SyntaxKind::Ident | SyntaxKind::QuotedIdent)
            {
                p.bump();
                p.bump();
            }
        }
    }
    if p.at(SyntaxKind::LParen) {
        p.bump();
        loop {
            if !(p.eat(SyntaxKind::Number) || p.eat(SyntaxKind::Ident)) {
                p.error("expected a type modifier");
                break;
            }
            if !p.eat(SyntaxKind::Comma) {
                break;
            }
        }
        p.expect(SyntaxKind::RParen, ")");
    }
    if matches!(first.as_str(), "timestamp" | "time")
        && (p.at_kw("with") || p.at_kw("without"))
        && p.nth_kw(1, "time")
    {
        p.bump();
        p.bump();
        p.expect_kw("zone");
    }
    while p.at(SyntaxKind::LBracket) {
        p.bump();
        p.eat(SyntaxKind::Number);
        p.expect(SyntaxKind::RBracket, "]");
    }
    m.complete(p, SyntaxKind::TypeName);
}
