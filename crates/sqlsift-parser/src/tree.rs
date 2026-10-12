//! The concrete syntax tree: every token of the input, trivia included, under nodes.
//!
//! Nodes live in an [`Arena`]: a node's children are one slice allocated there, so building
//! a tree is a few bump allocations instead of one `Vec` per node, and dropping it is free.
//! Reusing the arena (`Arena::reset`) between files reuses its memory. See #257.

use crate::SyntaxKind;

/// The memory a tree lives in. One arena can hold many trees; [`Arena::reset`] frees them all
/// at once and keeps the memory for the next ones.
#[derive(Default)]
pub struct Arena(bumpalo::Bump);

impl Arena {
    pub fn new() -> Self {
        Arena::default()
    }

    /// Frees every tree in the arena, keeping its memory for reuse.
    pub fn reset(&mut self) {
        self.0.reset();
    }

    /// Bytes allocated for trees so far.
    pub fn allocated_bytes(&self) -> usize {
        self.0.allocated_bytes()
    }

    pub(crate) fn alloc_children<'a>(
        &'a self,
        children: std::vec::Drain<'_, SyntaxElement<'a>>,
    ) -> &'a [SyntaxElement<'a>] {
        self.0.alloc_slice_fill_iter(children)
    }
}

impl std::fmt::Debug for Arena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arena")
            .field("allocated_bytes", &self.allocated_bytes())
            .finish()
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxToken {
    pub kind: SyntaxKind,
    pub range: TextRange,
}

impl SyntaxToken {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.to_range()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxElement<'a> {
    Node(SyntaxNode<'a>),
    Token(SyntaxToken),
}

impl SyntaxElement<'_> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxNode<'a> {
    pub kind: SyntaxKind,
    pub range: TextRange,
    pub children: &'a [SyntaxElement<'a>],
}

impl<'a> SyntaxNode<'a> {
    pub fn text<'s>(&self, source: &'s str) -> &'s str {
        &source[self.range.to_range()]
    }

    /// Child nodes, in order.
    pub fn nodes(&self) -> impl Iterator<Item = &'a SyntaxNode<'a>> {
        self.children.iter().filter_map(|c| match c {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(_) => None,
        })
    }

    /// Child tokens other than trivia, in order.
    pub fn tokens(&self) -> impl Iterator<Item = &'a SyntaxToken> {
        self.children.iter().filter_map(|c| match c {
            SyntaxElement::Token(t) if !t.kind.is_trivia() => Some(t),
            _ => None,
        })
    }

    /// The first child node of `kind`.
    pub fn child(&self, kind: SyntaxKind) -> Option<&'a SyntaxNode<'a>> {
        self.nodes().find(|n| n.kind == kind)
    }

    /// Whether this node or a node below it is an `Error` node.
    pub fn has_errors(&self) -> bool {
        self.kind == SyntaxKind::Error || self.nodes().any(SyntaxNode::has_errors)
    }

    /// All tokens below this node, trivia included, in order.
    pub fn descendant_tokens(&self) -> Vec<&'a SyntaxToken> {
        let mut out = Vec::new();
        self.collect_tokens(&mut out);
        out
    }

    fn collect_tokens(&self, out: &mut Vec<&'a SyntaxToken>) {
        for child in self.children {
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
        for child in self.children {
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
