//! An experimental PostgreSQL parser for sqlsift: a lossless concrete syntax tree (every
//! token, comments and whitespace included, with exact byte ranges) that is always built,
//! whatever the input. Syntax errors become `Error` nodes, and statements the grammar
//! doesn't cover yet become `OpaqueStmt` nodes, so one bad construct doesn't take the rest
//! of the file with it.
//!
//! Not used by sqlsift yet. [`lower`] turns statements into sqlparser's AST, so the
//! analyzer can run on this parser's output unchanged, and the differential test compares
//! the two parsers on the same input. See issue #257.

mod grammar;
pub mod lexer;
pub mod lower;
mod parser;
mod syntax_kind;
pub mod tree;

pub use parser::SyntaxError;
pub use syntax_kind::SyntaxKind;
pub use tree::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange};

/// The result of parsing a source file: the tree (always) and the syntax errors.
#[derive(Debug, Clone)]
pub struct Parse {
    pub root: SyntaxNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// The statements of the file, in order.
    pub fn statements(&self) -> impl Iterator<Item = &SyntaxNode> {
        self.root.nodes().filter(|n| n.kind != SyntaxKind::Error)
    }
}

/// Parses PostgreSQL source text.
pub fn parse(text: &str) -> Parse {
    let (tokens, lex_errors) = lexer::tokenize(text);
    let mut p = parser::Parser::new(text, &tokens);
    grammar::source_file(&mut p);
    let (root, mut errors) = parser::build_tree(&tokens, p.finish());
    errors.extend(lex_errors.into_iter().map(|e| {
        let at = u32::try_from(e.offset).unwrap_or(u32::MAX);
        SyntaxError {
            range: TextRange::new(at, at),
            message: e.message.to_string(),
        }
    }));
    errors.sort_by_key(|e| e.range.start);
    Parse { root, errors }
}
