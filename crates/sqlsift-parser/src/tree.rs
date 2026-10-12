//! The concrete syntax tree: every token of the input, trivia included, under nodes.
//!
//! This is a plain owned tree to start with. The parser only emits events
//! ([`crate::parser::Event`]), so the representation can change (an arena per statement,
//! see #257) without touching the grammar.

use crate::SyntaxKind;

/// A byte range in the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    pub fn new(start: u32, end: u32) -> Self {
        TextRange { start, end }
    }

    pub fn len(self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The range as `usize` indices into the source text.
    pub fn to_range(self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    pub kind: SyntaxKind,
    pub range: TextRange,
}

impl SyntaxToken {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.to_range()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxElement {
    Node(SyntaxNode),
    Token(SyntaxToken),
}

impl SyntaxElement {
    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Node(n) => n.kind,
            SyntaxElement::Token(t) => t.kind,
        }
    }

    pub fn range(&self) -> TextRange {
        match self {
            SyntaxElement::Node(n) => n.range,
            SyntaxElement::Token(t) => t.range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode {
    pub kind: SyntaxKind,
    pub range: TextRange,
    pub children: Vec<SyntaxElement>,
}

impl SyntaxNode {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.to_range()]
    }

    /// Child nodes, in order.
    pub fn nodes(&self) -> impl Iterator<Item = &SyntaxNode> {
        self.children.iter().filter_map(|c| match c {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(_) => None,
        })
    }

    /// Child tokens other than trivia, in order.
    pub fn tokens(&self) -> impl Iterator<Item = &SyntaxToken> {
        self.children.iter().filter_map(|c| match c {
            SyntaxElement::Token(t) if !t.kind.is_trivia() => Some(t),
            _ => None,
        })
    }

    /// The first child node of `kind`.
    pub fn child(&self, kind: SyntaxKind) -> Option<&SyntaxNode> {
        self.nodes().find(|n| n.kind == kind)
    }

    /// Whether this node or a node below it is an `Error` node.
    pub fn has_errors(&self) -> bool {
        self.kind == SyntaxKind::Error || self.nodes().any(SyntaxNode::has_errors)
    }

    /// All tokens below this node, trivia included, in order.
    pub fn descendant_tokens(&self) -> Vec<&SyntaxToken> {
        let mut out = Vec::new();
        self.collect_tokens(&mut out);
        out
    }

    fn collect_tokens<'a>(&'a self, out: &mut Vec<&'a SyntaxToken>) {
        for child in &self.children {
            match child {
                SyntaxElement::Node(n) => n.collect_tokens(out),
                SyntaxElement::Token(t) => out.push(t),
            }
        }
    }

    /// An indented dump of the tree, for tests and debugging.
    pub fn debug_dump(&self, source: &str) -> String {
        let mut out = String::new();
        self.dump(source, 0, &mut out);
        out
    }

    fn dump(&self, source: &str, depth: usize, out: &mut String) {
        use std::fmt::Write;
        let _ = writeln!(out, "{:indent$}{:?}", "", self.kind, indent = depth * 2);
        for child in &self.children {
            match child {
                SyntaxElement::Node(n) => n.dump(source, depth + 1, out),
                SyntaxElement::Token(t) if t.kind.is_trivia() => {}
                SyntaxElement::Token(t) => {
                    let _ = writeln!(
                        out,
                        "{:indent$}{:?} {:?}",
                        "",
                        t.kind,
                        t.text(source),
                        indent = (depth + 1) * 2
                    );
                }
            }
        }
    }
}
