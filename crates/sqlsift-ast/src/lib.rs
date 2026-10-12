//! The syntax tree sqlsift's analyzer reads (experimental, #257).
//!
//! Its types are the part of sqlparser's AST the analyzer and the schema builder use, with
//! the same names and fields, so either parser can produce it: sqlparser's AST is converted
//! to it (`sqlsift_core::convert`), and the in-house PostgreSQL parser lowers its syntax
//! tree to it. What the analyzer doesn't read is left out; an expression or statement it
//! doesn't model is [`Expr::Other`] / [`Statement::Other`], which keeps the expressions in
//! it so that walks over the tree still see them.

// The types mirror sqlparser's, enum variant sizes and boolean flags included
#![allow(clippy::large_enum_variant, clippy::struct_excessive_bools)]

mod data_type;
mod ddl;
mod display;
mod dml;
mod expr;
mod query;
mod span;
mod spans;
mod value;
pub mod visit;

pub use data_type::*;
pub use ddl::*;
pub use dml::*;
pub use expr::*;
pub use query::*;
pub use span::{AttachedToken, Location, Span, TokenWithSpan};
pub use spans::Spanned;
pub use value::*;
pub use visit::{visit_expressions, visit_relations, Visit, Visitor};

/// An identifier: a name, its quote character if it was quoted, and its location.
///
/// Identifiers compare equal whatever their locations.
#[derive(Debug, Clone, Eq)]
pub struct Ident {
    pub value: String,
    pub quote_style: Option<char>,
    pub span: Span,
}

impl Ident {
    pub fn new(value: impl Into<String>) -> Self {
        Ident {
            value: value.into(),
            quote_style: None,
            span: Span::empty(),
        }
    }

    pub fn with_span(span: Span, value: impl Into<String>) -> Self {
        Ident {
            value: value.into(),
            quote_style: None,
            span,
        }
    }

    pub fn with_quote(quote: char, value: impl Into<String>) -> Self {
        Ident {
            value: value.into(),
            quote_style: Some(quote),
            span: Span::empty(),
        }
    }
}

impl PartialEq for Ident {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.quote_style == other.quote_style
    }
}

impl std::hash::Hash for Ident {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
        self.quote_style.hash(state);
    }
}

impl From<&str> for Ident {
    fn from(value: &str) -> Self {
        Ident::new(value)
    }
}

/// A possibly qualified name: `schema.table`, `db.schema.table`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectName(pub Vec<ObjectNamePart>);

/// A part of an [`ObjectName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ObjectNamePart {
    Identifier(Ident),
    /// A part written as a function call (Snowflake `IDENTIFIER('t')`): its name and
    /// arguments.
    Function(ObjectNamePartFunction),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectNamePartFunction {
    pub name: Ident,
    pub args: Vec<FunctionArg>,
}

impl ObjectNamePart {
    /// The identifier of the part (`None` for a function part).
    pub fn as_ident(&self) -> Option<&Ident> {
        match self {
            ObjectNamePart::Identifier(ident) => Some(ident),
            ObjectNamePart::Function(_) => None,
        }
    }
}

impl From<Vec<Ident>> for ObjectName {
    fn from(idents: Vec<Ident>) -> Self {
        ObjectName(idents.into_iter().map(ObjectNamePart::Identifier).collect())
    }
}

/// `ident AS alias` (BigQuery `SELECT * RENAME (...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentWithAlias {
    pub ident: Ident,
    pub alias: Ident,
}

/// A value in parentheses, with the locations of the parentheses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parens<T> {
    pub opening_token: AttachedToken,
    pub content: T,
    pub closing_token: AttachedToken,
}

impl<T> std::ops::Deref for Parens<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.content
    }
}

impl<T> std::ops::DerefMut for Parens<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.content
    }
}
