//! What each query of a file returns (`sqlsift describe`): its output columns
//! and their types, as far as the schema tells

use std::ops::ControlFlow;

use sqlsift_ast::{Expr, SetExpr, Statement, Visit, Visitor};

use super::parameters::{placeholder_location, ParameterType};
use super::scope::{Column, ExpressionType, UNNAMED_COLUMN};
use crate::dialect::SqlDialect;
use crate::error::Diagnostic;
use crate::types::SqlType;

/// The queries of a file and the diagnostics found in it, from
/// [`Analyzer::describe`](super::Analyzer::describe)
#[derive(Debug, Clone, Default)]
pub struct Description {
    /// The SELECT, INSERT, UPDATE and DELETE statements, in source order
    pub queries: Vec<QueryDescription>,
    /// Diagnostics of the file, as [`Analyzer::analyze`](super::Analyzer::analyze)
    /// reports them
    pub diagnostics: Vec<Diagnostic>,
}

/// One statement and what it returns
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct QueryDescription {
    /// 1-indexed line where the statement starts
    pub line: usize,
    /// 1-indexed character column where the statement starts
    pub column: usize,
    /// Query name (sqlc `-- name: GetUser :one`, aiosql, HugSQL)
    pub name: Option<String>,
    /// What kind of statement it is
    pub kind: QueryKind,
    /// Output columns, or `None` if they can't be determined (`SELECT *` over an
    /// unknown table, a template that may add columns). INSERT, UPDATE and DELETE
    /// without RETURNING return no columns.
    pub columns: Option<Vec<OutputColumn>>,
    /// Parameters, by index
    pub parameters: Vec<Parameter>,
}

/// Kind of a described statement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// A query: SELECT, VALUES, WITH ... SELECT, set operations
    Select,
    /// INSERT
    Insert,
    /// UPDATE
    Update,
    /// DELETE
    Delete,
}

impl QueryKind {
    /// Kind of a statement that is described, `None` for others (DDL, SET, ...)
    pub(super) fn of(stmt: &Statement) -> Option<Self> {
        match stmt {
            Statement::Query(query) => match &*query.body {
                // `WITH ... INSERT` is a query whose body is the statement
                SetExpr::Insert(stmt) | SetExpr::Update(stmt) | SetExpr::Delete(stmt) => {
                    Self::of(stmt)
                }
                _ => Some(Self::Select),
            },
            Statement::Insert(_) => Some(Self::Insert),
            Statement::Update(_) => Some(Self::Update),
            Statement::Delete(_) => Some(Self::Delete),
            _ => None,
        }
    }

    /// Lowercase name (`select`, `insert`, `update`, `delete`)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// Whether a DML statement has a RETURNING clause (queries always return rows)
pub(super) fn has_returning(stmt: &Statement) -> bool {
    match stmt {
        Statement::Insert(insert) => insert.returning.is_some(),
        Statement::Update(update) => update.returning.is_some(),
        Statement::Delete(delete) => delete.returning.is_some(),
        Statement::Query(query) => match &*query.body {
            SetExpr::Insert(stmt) | SetExpr::Update(stmt) | SetExpr::Delete(stmt) => {
                has_returning(stmt)
            }
            _ => true,
        },
        _ => true,
    }
}

/// An output column of a query
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OutputColumn {
    /// Column name, or `None` if it can't be told: an unaliased expression the
    /// dialect names after its text (MySQL, SQLite) or PostgreSQL names `?column?`
    /// or after a type
    pub name: Option<String>,
    /// Column type, or `None` if it can't be inferred
    pub ty: Option<SqlType>,
    /// Whether the column may be NULL: `Some(false)` only when it certainly isn't
    /// (a NOT NULL column on the required side of every join, a literal,
    /// `count(*)`, ...), `None` if it can't be told
    pub nullable: Option<bool>,
}

impl OutputColumn {
    pub(super) fn new(column: Column, dialect: SqlDialect) -> Self {
        // PostgreSQL's rules name `count(*)` `count` and `x::int` `x`; other
        // dialects name an unaliased expression after its text
        let name = (column.name != UNNAMED_COLUMN && (!column.guessed_name || dialect.is_psql()))
            .then(|| column.name.clone());
        let ty = match column.ty {
            ExpressionType::Known(SqlType::Unknown) | ExpressionType::Unknown => None,
            ty => Some(ty.to_sql_type()),
        };
        Self {
            name,
            ty,
            nullable: column.nullable,
        }
    }
}

/// A parameter of a query (`$1`, `?`, `:name`, `@name`, `sqlc.arg(name)`)
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Parameter {
    /// 1-indexed position: the number of `$1` / `?1`, else the order in which
    /// the parameter first appears (each `?` and each template interpolation is
    /// its own parameter; a named parameter used twice is one)
    pub index: usize,
    /// Name of a named parameter (`:id`, `@id`, `sqlc.arg(id)`, psql `:'id'`)
    pub name: Option<String>,
    /// Type of the values it takes, or `None` if it can't be inferred (not
    /// compared with or assigned to anything typed, or used where different types
    /// are expected)
    pub ty: Option<SqlType>,
}

/// How a placeholder names its parameter
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ParameterKey {
    Numbered(usize),
    Named(String),
    /// `?` or a template interpolation: a parameter of its own
    Positional,
}

/// How the placeholder written at the start of `text` names its parameter
fn parameter_key(text: &str) -> ParameterKey {
    fn word(text: &str) -> &str {
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        &text[..end]
    }
    fn number(text: &str) -> Option<usize> {
        let digits = &text[..text
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len())];
        digits.parse().ok()
    }
    let named = |name: &str| {
        if name.is_empty() {
            ParameterKey::Positional
        } else {
            ParameterKey::Named(name.to_string())
        }
    };
    if let Some(rest) = text.strip_prefix('$').or_else(|| text.strip_prefix('?')) {
        if let Some(n) = number(rest) {
            return ParameterKey::Numbered(n);
        }
        // `${expr}` (template literal) and `?`
        if rest.starts_with('{') || text.starts_with('?') {
            return ParameterKey::Positional;
        }
        return named(word(rest)); // SQLite `$name`
    }
    if let Some(rest) = text.strip_prefix(':').or_else(|| text.strip_prefix('@')) {
        // psql `:'name'`
        if let Some(quoted) = rest.strip_prefix('\'') {
            return named(word(quoted));
        }
        // aiosql / HugSQL `:v*:ids`, `:i:col`
        let first = word(rest);
        if let Some(after) = rest[first.len()..]
            .strip_prefix("*:")
            .or_else(|| rest[first.len()..].strip_prefix(':'))
        {
            return named(word(after));
        }
        return named(first);
    }
    // sqlc.arg(name), sqlc.narg('name'), sqlc.slice(name)
    if let Some(rest) = text.strip_prefix("sqlc.") {
        let open = rest.find('(').map_or("", |i| &rest[i + 1..]);
        let open = open.trim_start().trim_start_matches(['\'', '"']);
        return named(word(open));
    }
    ParameterKey::Positional
}

/// The parameters of a statement from its placeholder occurrences, in source
/// order: the text each starts with and the type inferred for it
pub(super) fn parameters<'t>(
    occurrences: impl IntoIterator<Item = (&'t str, Option<ParameterType>)>,
) -> Vec<Parameter> {
    let mut keys: Vec<ParameterKey> = Vec::new();
    let mut types: Vec<Option<ParameterType>> = Vec::new();
    for (text, ty) in occurrences {
        let key = parameter_key(text);
        let existing = (key != ParameterKey::Positional)
            .then(|| keys.iter().position(|k| *k == key))
            .flatten();
        if let Some(i) = existing {
            types[i] = match (types[i].take(), ty) {
                (Some(a), Some(b)) => Some(a.merge(b)),
                (a, b) => a.or(b),
            };
        } else {
            keys.push(key);
            types.push(ty);
        }
    }
    let mut next = 0;
    let mut parameters: Vec<Parameter> = keys
        .into_iter()
        .zip(types)
        .map(|(key, ty)| {
            let ty = match ty {
                Some(ParameterType::Known(ty)) => Some(ty),
                _ => None,
            };
            let (index, name) = match key {
                ParameterKey::Numbered(n) => (n, None),
                ParameterKey::Named(name) => {
                    next += 1;
                    (next, Some(name))
                }
                ParameterKey::Positional => {
                    next += 1;
                    (next, None)
                }
            };
            Parameter { index, name, ty }
        })
        .collect();
    parameters.sort_by_key(|p| p.index);
    parameters
}

/// Locations ((line, column) relative to the statement's text) of the
/// placeholders of `stmt`, in source order
pub(super) fn placeholder_locations(stmt: &Statement) -> Vec<(usize, usize)> {
    struct Placeholders(Vec<(usize, usize)>);
    impl Visitor for Placeholders {
        type Break = ();
        fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
            if matches!(expr, Expr::Value(_)) {
                if let Some(location) = placeholder_location(expr) {
                    self.0.push(location);
                }
            }
            ControlFlow::Continue(())
        }
    }
    let mut placeholders = Placeholders(Vec::new());
    let _ = stmt.visit(&mut placeholders);
    placeholders.0.sort_unstable();
    placeholders.0.dedup();
    placeholders.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_keys() {
        use ParameterKey::{Named, Numbered, Positional};
        assert_eq!(parameter_key("$12 AND"), Numbered(12));
        assert_eq!(parameter_key("?3"), Numbered(3));
        assert_eq!(parameter_key("? AND"), Positional);
        assert_eq!(parameter_key("${user.id}`"), Positional);
        assert_eq!(parameter_key("$name"), Named("name".into()));
        assert_eq!(parameter_key(":id)"), Named("id".into()));
        assert_eq!(parameter_key(":'id'"), Named("id".into()));
        assert_eq!(parameter_key(":v*:ids)"), Named("ids".into()));
        assert_eq!(parameter_key(":i:col "), Named("col".into()));
        assert_eq!(parameter_key("@user_id "), Named("user_id".into()));
        assert_eq!(parameter_key("sqlc.arg(user_id)"), Named("user_id".into()));
        assert_eq!(parameter_key("sqlc.narg('name')"), Named("name".into()));
    }

    #[test]
    fn named_parameters_are_numbered_in_order_and_merged() {
        let int = || Some(ParameterType::Known(SqlType::Integer));
        let params = parameters([(":b", int()), (":a", None), (":b", int()), ("?", None)]);
        let summary: Vec<(usize, Option<&str>, bool)> = params
            .iter()
            .map(|p| (p.index, p.name.as_deref(), p.ty.is_some()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (1, Some("b"), true),
                (2, Some("a"), false),
                (3, None, false)
            ]
        );
    }

    #[test]
    fn conflicting_types_are_unknown() {
        let params = parameters([
            ("$1", Some(ParameterType::Known(SqlType::Integer))),
            ("$1", Some(ParameterType::Known(SqlType::Text))),
            ("$2", Some(ParameterType::Known(SqlType::Text))),
        ]);
        assert_eq!(params[0].ty, None);
        assert_eq!(params[1].ty, Some(SqlType::Text));
    }
}
