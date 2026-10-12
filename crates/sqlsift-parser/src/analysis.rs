//! A source file's statements for the analyzer: each one lowered to sqlparser's AST,
//! left to sqlparser, or a syntax error.

use sqlparser::ast::Statement;

use crate::lower::lower_statement;
use crate::{parse, Arena, SyntaxError, SyntaxKind, SyntaxNode, TextRange};
use std::cell::RefCell;

/// What became of one statement.
#[derive(Debug)]
pub enum Outcome {
    /// Lowered, with locations relative to the whole source.
    Lowered(Box<Statement>),
    /// Parsed, but the lowering doesn't handle it (the reason).
    Unsupported(String),
    /// The first syntax error in the statement.
    SyntaxError(SyntaxError),
}

/// One statement: its range in the source (without the `;`) and its outcome.
#[derive(Debug)]
pub struct AnalyzedStatement {
    pub range: TextRange,
    pub outcome: Outcome,
}

/// Parses `source` once and splits it at the top-level `;`s (so text the parser couldn't
/// place stays with its statement).
///
/// The tree goes in a per-thread arena that is reset on each call, so parsing file after file
/// reuses the same memory.
pub fn statements(source: &str) -> Vec<AnalyzedStatement> {
    thread_local! {
        static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
    }
    ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.reset();
        statements_in(&arena, source)
    })
}

fn statements_in(arena: &Arena, source: &str) -> Vec<AnalyzedStatement> {
    let parsed = parse(arena, source);
    let mut out = vec![];
    let mut nodes: Vec<&SyntaxNode> = vec![];
    let mut range: Option<TextRange> = None;
    let mut flush = |range: &mut Option<TextRange>, nodes: &mut Vec<&SyntaxNode>| {
        let Some(r) = range.take() else {
            return;
        };
        let error = parsed
            .errors
            .iter()
            .find(|e| r.start <= e.range.start && e.range.start <= r.end);
        let outcome = match (error, &nodes[..]) {
            (Some(e), _) => Outcome::SyntaxError(e.clone()),
            (None, [node]) => match lower_statement(source, node) {
                Ok(statement) => Outcome::Lowered(Box::new(statement)),
                Err(e) => Outcome::Unsupported(e.0),
            },
            (None, _) => Outcome::Unsupported("not one statement".into()),
        };
        out.push(AnalyzedStatement { range: r, outcome });
        nodes.clear();
    };
    for child in parsed.root.children {
        let kind = child.kind();
        if kind == SyntaxKind::Semicolon {
            flush(&mut range, &mut nodes);
        } else if !kind.is_trivia() {
            let r = child.range();
            range = Some(match range {
                Some(prev) => TextRange::new(prev.start, r.end),
                None => r,
            });
            if let crate::SyntaxElement::Node(node) = child {
                nodes.push(node);
            }
        }
    }
    flush(&mut range, &mut nodes);
    out
}
