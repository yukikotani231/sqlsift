//! Parser infrastructure: a cursor over the non-trivia tokens that records events
//! (start node, token, finish node, error), and the step that turns events into a tree.
//!
//! Same design as rust-analyzer's parser: grammar functions never build nodes directly, so
//! a node can be wrapped after the fact ([`CompletedMarker::precede`], used for binary
//! expressions and joins) and the tree representation is independent of the grammar.

use crate::lexer::Token;
use crate::tree::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange};
use crate::SyntaxKind;

/// A syntax error: where, and what was expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

#[derive(Debug)]
pub(crate) enum Event {
    Start {
        kind: SyntaxKind,
        /// Index of an event to start as this node's parent (set by `precede`).
        forward_parent: Option<usize>,
    },
    Finish,
    Token,
    Error(String),
    /// An abandoned or already-forwarded `Start`.
    Tombstone,
}

pub(crate) struct Parser<'t> {
    text: &'t str,
    /// Non-trivia tokens with their ranges.
    tokens: Vec<(SyntaxKind, TextRange)>,
    pos: usize,
    events: Vec<Event>,
    /// Guards against grammar loops that make no progress.
    steps: u32,
}

const EOF: SyntaxKind = SyntaxKind::Unknown;

impl<'t> Parser<'t> {
    pub(crate) fn new(text: &'t str, raw: &[Token]) -> Self {
        let mut offset = 0u32;
        let mut tokens = Vec::with_capacity(raw.len());
        for t in raw {
            let range = TextRange::new(offset, offset + t.len);
            offset += t.len;
            if !t.kind.is_trivia() {
                tokens.push((t.kind, range));
            }
        }
        Parser {
            text,
            tokens,
            pos: 0,
            events: Vec::new(),
            steps: 0,
        }
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    // --- lookahead ---

    /// The kind of the n-th token ahead. At the end of input this is `Unknown` with an
    /// empty text, which no grammar rule accepts.
    pub(crate) fn nth(&mut self, n: usize) -> SyntaxKind {
        self.steps += 1;
        assert!(self.steps < 10_000_000, "parser is stuck");
        self.tokens.get(self.pos + n).map_or(EOF, |t| t.0)
    }

    pub(crate) fn current(&mut self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    pub(crate) fn at(&mut self, kind: SyntaxKind) -> bool {
        !self.at_end() && self.current() == kind
    }

    pub(crate) fn nth_text(&self, n: usize) -> &'t str {
        self.tokens
            .get(self.pos + n)
            .map_or("", |t| &self.text[t.1.to_range()])
    }

    /// Whether the n-th token ahead is the keyword `kw` (an unquoted identifier, any case).
    pub(crate) fn nth_kw(&mut self, n: usize, kw: &str) -> bool {
        self.nth(n) == SyntaxKind::Ident && self.nth_text(n).eq_ignore_ascii_case(kw)
    }

    pub(crate) fn at_kw(&mut self, kw: &str) -> bool {
        self.nth_kw(0, kw)
    }

    pub(crate) fn at_any_kw(&mut self, kws: &[&str]) -> bool {
        kws.iter().any(|kw| self.nth_kw(0, kw))
    }

    /// Whether the current token is the operator `op`.
    pub(crate) fn at_op(&mut self, op: &str) -> bool {
        self.nth_op(0, op)
    }

    pub(crate) fn nth_op(&mut self, n: usize, op: &str) -> bool {
        self.nth(n) == SyntaxKind::Operator && self.nth_text(n) == op
    }

    // --- consuming ---

    pub(crate) fn bump(&mut self) {
        assert!(!self.at_end(), "bump at end of input");
        self.pos += 1;
        self.events.push(Event::Token);
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind, what: &str) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.error(format!("expected {what}"));
            false
        }
    }

    pub(crate) fn expect_kw(&mut self, kw: &str) -> bool {
        if self.eat_kw(kw) {
            true
        } else {
            self.error(format!("expected {}", kw.to_ascii_uppercase()));
            false
        }
    }

    /// Records an error at the current token without consuming it.
    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.events.push(Event::Error(message.into()));
    }

    /// Records an error and wraps the current token in an `Error` node.
    pub(crate) fn err_and_bump(&mut self, message: impl Into<String>) {
        self.error(message);
        if !self.at_end() {
            let m = self.start();
            self.bump();
            m.complete(self, SyntaxKind::Error);
        }
    }

    // --- nodes ---

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len();
        self.events.push(Event::Start {
            kind: SyntaxKind::Error,
            forward_parent: None,
        });
        Marker { pos, done: false }
    }
}

/// An open node. It must be completed or abandoned.
pub(crate) struct Marker {
    pos: usize,
    done: bool,
}

impl Marker {
    pub(crate) fn complete(mut self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        self.done = true;
        match &mut p.events[self.pos] {
            Event::Start { kind: k, .. } => *k = kind,
            _ => unreachable!("marker does not point at a Start event"),
        }
        p.events.push(Event::Finish);
        CompletedMarker { pos: self.pos }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        assert!(
            self.done || std::thread::panicking(),
            "marker was never completed"
        );
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: usize,
}

impl CompletedMarker {
    /// Starts a node that will contain this completed one.
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let m = p.start();
        match &mut p.events[self.pos] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(m.pos),
            _ => unreachable!("completed marker does not point at a Start event"),
        }
        m
    }
}

/// Builds the tree from the events and all tokens (trivia included). Trivia before a node
/// is placed in the enclosing node, so nodes start and end with a real token.
pub(crate) fn build_tree(raw: &[Token], mut events: Vec<Event>) -> (SyntaxNode, Vec<SyntaxError>) {
    let mut stack: Vec<SyntaxNode> = vec![];
    let mut errors = Vec::new();
    let mut raw_pos = 0usize;
    let mut offset = 0u32;

    let push_trivia = |stack: &mut Vec<SyntaxNode>, raw_pos: &mut usize, offset: &mut u32| {
        while let Some(t) = raw.get(*raw_pos) {
            if !t.kind.is_trivia() {
                break;
            }
            let range = TextRange::new(*offset, *offset + t.len);
            *offset += t.len;
            *raw_pos += 1;
            if let Some(top) = stack.last_mut() {
                top.children.push(SyntaxElement::Token(SyntaxToken {
                    kind: t.kind,
                    range,
                }));
            }
        }
    };

    let mut i = 0;
    while i < events.len() {
        match std::mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                // Collect the chain of forward parents: outermost first.
                let mut kinds = vec![kind];
                let mut next = forward_parent;
                while let Some(idx) = next {
                    match std::mem::replace(&mut events[idx], Event::Tombstone) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            kinds.push(kind);
                            next = forward_parent;
                        }
                        _ => unreachable!("forward parent is not a Start event"),
                    }
                }
                if !stack.is_empty() {
                    push_trivia(&mut stack, &mut raw_pos, &mut offset);
                }
                for kind in kinds.into_iter().rev() {
                    stack.push(SyntaxNode {
                        kind,
                        range: TextRange::new(offset, offset),
                        children: vec![],
                    });
                }
            }
            Event::Finish => {
                let mut node = stack.pop().expect("unbalanced Finish event");
                if stack.is_empty() {
                    // The root takes the trailing trivia.
                    stack.push(node);
                    push_trivia(&mut stack, &mut raw_pos, &mut offset);
                    node = stack.pop().expect("root");
                    node.range = TextRange::new(0, offset);
                    return (node, errors);
                }
                node.range.end = node
                    .children
                    .last()
                    .map_or(node.range.start, |c| c.range().end);
                if let Some(first) = node.children.first() {
                    node.range.start = first.range().start;
                }
                stack
                    .last_mut()
                    .expect("parent")
                    .children
                    .push(SyntaxElement::Node(node));
            }
            Event::Token => {
                push_trivia(&mut stack, &mut raw_pos, &mut offset);
                let t = raw[raw_pos];
                let range = TextRange::new(offset, offset + t.len);
                offset += t.len;
                raw_pos += 1;
                stack
                    .last_mut()
                    .expect("token outside a node")
                    .children
                    .push(SyntaxElement::Token(SyntaxToken {
                        kind: t.kind,
                        range,
                    }));
            }
            Event::Error(message) => {
                // Point the error at the next non-trivia token (or the end of input).
                let mut at = offset;
                let mut len = 0;
                for t in &raw[raw_pos..] {
                    if !t.kind.is_trivia() {
                        len = t.len;
                        break;
                    }
                    at += t.len;
                }
                errors.push(SyntaxError {
                    range: TextRange::new(at, at + len),
                    message,
                });
            }
            Event::Tombstone => {}
        }
        i += 1;
    }
    unreachable!("the root node was never finished")
}
