//! Source locations, as sqlparser gives them: 1-based line and character column, `0:0`
//! for an unknown location.

/// A 1-based line and character column. `0:0` is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Location {
    pub line: u64,
    pub column: u64,
}

impl Location {
    pub const fn new(line: u64, column: u64) -> Self {
        Location { line, column }
    }

    pub const fn empty() -> Self {
        Location { line: 0, column: 0 }
    }
}

/// The text between two locations (the end is exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Span {
    pub start: Location,
    pub end: Location,
}

impl Span {
    pub const fn new(start: Location, end: Location) -> Self {
        Span { start, end }
    }

    /// The unknown location.
    pub const fn empty() -> Self {
        Span {
            start: Location::empty(),
            end: Location::empty(),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Span::empty()
    }

    /// The smallest span covering both (an empty span is ignored).
    #[must_use]
    pub fn union(&self, other: &Span) -> Span {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => *other,
            (_, true) => *self,
            _ => Span {
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            },
        }
    }

    /// [`Span::union`] of an optional span.
    #[must_use]
    pub fn union_opt(&self, other: &Option<Span>) -> Span {
        match other {
            Some(other) => self.union(other),
            None => *self,
        }
    }

    /// The union of all the spans (empty if there are none).
    pub fn union_iter<I: IntoIterator<Item = Span>>(iter: I) -> Span {
        iter.into_iter()
            .reduce(|acc, item| acc.union(&item))
            .unwrap_or_else(Span::empty)
    }
}

/// A token's location, as part of a node (sqlparser's `TokenWithSpan`): only the span is
/// kept.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenWithSpan {
    pub span: Span,
}

/// A keyword or punctuation token attached to a node, kept for its location. Nodes compare
/// equal whatever their tokens' locations (as with sqlparser's `AttachedToken`).
#[derive(Debug, Clone, Copy, Default)]
pub struct AttachedToken(pub TokenWithSpan);

impl AttachedToken {
    pub const fn new(span: Span) -> Self {
        AttachedToken(TokenWithSpan { span })
    }

    pub const fn empty() -> Self {
        AttachedToken::new(Span::empty())
    }
}

impl PartialEq for AttachedToken {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for AttachedToken {}

impl std::hash::Hash for AttachedToken {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}
