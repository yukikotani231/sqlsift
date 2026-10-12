//! PostgreSQL lexer. Every byte of the input ends up in exactly one token, so the token
//! lengths add up to the input length (the tree built from them is lossless).
//!
//! Follows `scan.l` of PostgreSQL where it matters for splitting tokens: nested block
//! comments, dollar quoting, `E''` / `U&''` strings and the rule that an operator only
//! ends in `+` or `-` if it contains one of `~ ! @ # % ^ & | \` ?`.

use crate::SyntaxKind;

/// A token: its kind and length in bytes. Its position is the sum of the lengths before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: u32,
}

/// A malformed token (unterminated string or comment, unknown character).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub offset: usize,
    pub message: &'static str,
}

/// Splits `text` into tokens, trivia included.
pub fn tokenize(text: &str) -> (Vec<Token>, Vec<LexError>) {
    let mut lexer = Lexer {
        bytes: text.as_bytes(),
        pos: 0,
        errors: Vec::new(),
    };
    let mut tokens = Vec::new();
    while lexer.pos < lexer.bytes.len() {
        let start = lexer.pos;
        let kind = lexer.next_kind();
        debug_assert!(lexer.pos > start, "lexer made no progress");
        // A token can't be longer than u32::MAX: inputs that large are not SQL files.
        let len = u32::try_from(lexer.pos - start).unwrap_or(u32::MAX);
        tokens.push(Token { kind, len });
    }
    (tokens, lexer.errors)
}

const OP_CHARS: &[u8] = b"+-*/<>=~!@#%^&|`?";
const OP_SPECIAL: &[u8] = b"~!@#%^&|`?";

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    errors: Vec<LexError>,
}

impl Lexer<'_> {
    fn peek(&self, n: usize) -> u8 {
        self.bytes.get(self.pos + n).copied().unwrap_or(0)
    }

    fn error(&mut self, offset: usize, message: &'static str) {
        self.errors.push(LexError { offset, message });
    }

    fn next_kind(&mut self) -> SyntaxKind {
        let b = self.peek(0);
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                while matches!(self.peek(0), b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
                    self.pos += 1;
                }
                SyntaxKind::Whitespace
            }
            b'-' if self.peek(1) == b'-' => {
                while self.pos < self.bytes.len() && !matches!(self.peek(0), b'\n' | b'\r') {
                    self.pos += 1;
                }
                SyntaxKind::LineComment
            }
            b'/' if self.peek(1) == b'*' => self.block_comment(),
            b'\'' => {
                self.quoted(b'\'', false);
                SyntaxKind::String
            }
            b'"' => {
                self.quoted(b'"', false);
                SyntaxKind::QuotedIdent
            }
            b'e' | b'E' if self.peek(1) == b'\'' => {
                self.pos += 1;
                self.quoted(b'\'', true);
                SyntaxKind::EscapeString
            }
            b'n' | b'N' if self.peek(1) == b'\'' => {
                self.pos += 1;
                self.quoted(b'\'', false);
                SyntaxKind::NationalString
            }
            b'b' | b'B' if self.peek(1) == b'\'' => {
                self.pos += 1;
                self.quoted(b'\'', false);
                SyntaxKind::BitString
            }
            b'x' | b'X' if self.peek(1) == b'\'' => {
                self.pos += 1;
                self.quoted(b'\'', false);
                SyntaxKind::HexString
            }
            b'u' | b'U' if self.peek(1) == b'&' && matches!(self.peek(2), b'\'' | b'"') => {
                self.pos += 2;
                if self.peek(0) == b'\'' {
                    self.quoted(b'\'', false);
                    SyntaxKind::UnicodeString
                } else {
                    self.quoted(b'"', false);
                    SyntaxKind::QuotedIdent
                }
            }
            b'$' => self.dollar(),
            b'0'..=b'9' => self.number(),
            b'.' if self.peek(1).is_ascii_digit() => self.number(),
            _ if is_ident_start(b) => {
                while is_ident_continue(self.peek(0)) {
                    self.pos += 1;
                }
                SyntaxKind::Ident
            }
            b'(' => self.single(SyntaxKind::LParen),
            b')' => self.single(SyntaxKind::RParen),
            b'[' => self.single(SyntaxKind::LBracket),
            b']' => self.single(SyntaxKind::RBracket),
            b',' => self.single(SyntaxKind::Comma),
            b';' => self.single(SyntaxKind::Semicolon),
            b'.' => self.single(SyntaxKind::Dot),
            b':' => match self.peek(1) {
                b':' => {
                    self.pos += 2;
                    SyntaxKind::DoubleColon
                }
                b'=' => {
                    self.pos += 2;
                    SyntaxKind::Operator
                }
                _ => self.single(SyntaxKind::Colon),
            },
            _ if OP_CHARS.contains(&b) => self.operator(),
            _ => {
                let start = self.pos;
                // Skip a whole UTF-8 character (`is_ident_start` takes bytes >= 0x80, so
                // this is an ASCII control or punctuation character).
                self.pos += 1;
                self.error(start, "unexpected character");
                SyntaxKind::Unknown
            }
        }
    }

    fn single(&mut self, kind: SyntaxKind) -> SyntaxKind {
        self.pos += 1;
        kind
    }

    fn block_comment(&mut self) -> SyntaxKind {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1;
        while self.pos < self.bytes.len() {
            if self.peek(0) == b'/' && self.peek(1) == b'*' {
                depth += 1;
                self.pos += 2;
            } else if self.peek(0) == b'*' && self.peek(1) == b'/' {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    return SyntaxKind::BlockComment;
                }
            } else {
                self.pos += 1;
            }
        }
        self.error(start, "unterminated block comment");
        SyntaxKind::BlockComment
    }

    /// A quoted string or identifier starting at the quote; a doubled quote is an escaped
    /// quote, and with `backslash` so is `\'`.
    fn quoted(&mut self, quote: u8, backslash: bool) {
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() {
            let b = self.peek(0);
            if backslash && b == b'\\' {
                self.pos = (self.pos + 2).min(self.bytes.len());
            } else if b == quote {
                self.pos += 1;
                if self.peek(0) == quote {
                    self.pos += 1;
                } else {
                    return;
                }
            } else {
                self.pos += 1;
            }
        }
        self.error(
            start,
            if quote == b'"' {
                "unterminated quoted identifier"
            } else {
                "unterminated string literal"
            },
        );
    }

    fn dollar(&mut self) -> SyntaxKind {
        let start = self.pos;
        if self.peek(1).is_ascii_digit() {
            self.pos += 1;
            while self.peek(0).is_ascii_digit() {
                self.pos += 1;
            }
            return SyntaxKind::Param;
        }
        // `$tag$`: the tag is an identifier without `$`.
        let mut end = self.pos + 1;
        if is_ident_start(self.bytes.get(end).copied().unwrap_or(0)) {
            while self
                .bytes
                .get(end)
                .is_some_and(|&b| is_ident_continue(b) && b != b'$')
            {
                end += 1;
            }
        }
        if self.bytes.get(end) != Some(&b'$') {
            self.pos += 1;
            self.error(start, "unexpected character");
            return SyntaxKind::Unknown;
        }
        let tag = &self.bytes[start..=end];
        self.pos = end + 1;
        while self.pos < self.bytes.len() {
            if self.bytes[self.pos..].starts_with(tag) {
                self.pos += tag.len();
                return SyntaxKind::DollarString;
            }
            self.pos += 1;
        }
        self.error(start, "unterminated dollar-quoted string");
        SyntaxKind::DollarString
    }

    fn digits(&mut self, valid: fn(u8) -> bool) {
        while valid(self.peek(0)) || (self.peek(0) == b'_' && valid(self.peek(1))) {
            self.pos += 1;
        }
    }

    fn number(&mut self) -> SyntaxKind {
        if self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
            let valid: fn(u8) -> bool = match self.peek(1) {
                b'x' | b'X' => |b: u8| b.is_ascii_hexdigit(),
                b'o' | b'O' => |b: u8| (b'0'..=b'7').contains(&b),
                _ => |b: u8| b == b'0' || b == b'1',
            };
            if valid(self.peek(2)) {
                self.pos += 2;
                self.digits(valid);
                return SyntaxKind::Number;
            }
        }
        self.digits(|b| b.is_ascii_digit());
        // `1.5`, `1.` and `.5`, but not `1..2`.
        if self.peek(0) == b'.' && self.peek(1) != b'.' {
            self.pos += 1;
            self.digits(|b| b.is_ascii_digit());
        }
        if matches!(self.peek(0), b'e' | b'E') {
            let sign = usize::from(matches!(self.peek(1), b'+' | b'-'));
            if self.peek(1 + sign).is_ascii_digit() {
                self.pos += 1 + sign;
                self.digits(|b| b.is_ascii_digit());
            }
        }
        SyntaxKind::Number
    }

    fn operator(&mut self) -> SyntaxKind {
        let start = self.pos;
        let mut end = self.pos;
        while let Some(&b) = self.bytes.get(end) {
            if !OP_CHARS.contains(&b) {
                break;
            }
            // A comment start ends the operator.
            let next = self.bytes.get(end + 1).copied().unwrap_or(0);
            if end > start && ((b == b'-' && next == b'-') || (b == b'/' && next == b'*')) {
                break;
            }
            end += 1;
        }
        let op = &self.bytes[start..end];
        if op.len() > 1 && !op.iter().any(|b| OP_SPECIAL.contains(b)) {
            while end > start + 1 && matches!(self.bytes[end - 1], b'+' | b'-') {
                end -= 1;
            }
        }
        self.pos = end;
        SyntaxKind::Operator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(text: &str) -> Vec<(SyntaxKind, &str)> {
        let (tokens, _) = tokenize(text);
        let mut pos = 0;
        tokens
            .iter()
            .map(|t| {
                let s = &text[pos..pos + t.len as usize];
                pos += t.len as usize;
                (t.kind, s)
            })
            .filter(|(k, _)| *k != SyntaxKind::Whitespace)
            .collect()
    }

    #[test]
    fn operators_follow_postgres_rules() {
        use SyntaxKind::Operator as Op;
        assert_eq!(
            lex("a*-1"),
            [
                (SyntaxKind::Ident, "a"),
                (Op, "*"),
                (Op, "-"),
                (SyntaxKind::Number, "1")
            ]
        );
        assert_eq!(lex("a @- b")[1], (Op, "@-"));
        assert_eq!(lex("a<=b")[1], (Op, "<="));
        assert_eq!(lex("j->>'k'")[1], (Op, "->>"));
        assert_eq!(lex("a+-- c\n")[1], (Op, "+"));
        assert_eq!(lex("a::int")[1], (SyntaxKind::DoubleColon, "::"));
    }

    #[test]
    fn strings_and_quoting() {
        assert_eq!(lex("'it''s'"), [(SyntaxKind::String, "'it''s'")]);
        assert_eq!(lex(r"E'a\'b'"), [(SyntaxKind::EscapeString, r"E'a\'b'")]);
        assert_eq!(
            lex("$fn$ a $$ b $fn$"),
            [(SyntaxKind::DollarString, "$fn$ a $$ b $fn$")]
        );
        assert_eq!(lex("$$x$$"), [(SyntaxKind::DollarString, "$$x$$")]);
        assert_eq!(lex("$12"), [(SyntaxKind::Param, "$12")]);
        assert_eq!(lex(r#""a""b""#), [(SyntaxKind::QuotedIdent, r#""a""b""#)]);
        assert_eq!(
            lex(r#"U&"d\0061""#),
            [(SyntaxKind::QuotedIdent, r#"U&"d\0061""#)]
        );
        assert_eq!(lex("x'1f'")[0].0, SyntaxKind::HexString);
    }

    #[test]
    fn numbers() {
        for n in ["1", "1.5", ".5", "1.", "1e10", "1.5E-3", "0x1F", "1_000"] {
            assert_eq!(lex(n), [(SyntaxKind::Number, n)], "{n}");
        }
        assert_eq!(
            lex("1..2"),
            [
                (SyntaxKind::Number, "1"),
                (SyntaxKind::Dot, "."),
                (SyntaxKind::Number, ".2")
            ]
        );
    }

    #[test]
    fn comments_nest() {
        assert_eq!(
            lex("/* a /* b */ c */x"),
            [
                (SyntaxKind::BlockComment, "/* a /* b */ c */"),
                (SyntaxKind::Ident, "x")
            ]
        );
        let (_, errors) = tokenize("/* open");
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn identifiers_take_dollar_and_unicode() {
        assert_eq!(lex("a$b"), [(SyntaxKind::Ident, "a$b")]);
        assert_eq!(lex("名前"), [(SyntaxKind::Ident, "名前")]);
    }
}
