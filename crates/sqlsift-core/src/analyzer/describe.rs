//! What each query of a file returns (`sqlsift describe`): its output columns
//! and their types, as far as the schema tells

use sqlparser::ast::{SetExpr, Statement};

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
}

impl OutputColumn {
    pub(super) fn new(column: Column, dialect: SqlDialect) -> Self {
        // PostgreSQL's rules name `count(*)` `count` and `x::int` `x`; other
        // dialects name an unaliased expression after its text
        let name = (column.name != UNNAMED_COLUMN && (!column.guessed_name || dialect.is_psql()))
            .then_some(column.name);
        let ty = match column.ty {
            ExpressionType::Known(SqlType::Unknown) | ExpressionType::Unknown => None,
            ty => Some(ty.to_sql_type()),
        };
        Self { name, ty }
    }
}
