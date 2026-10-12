//! Small accessors over the sqlparser AST.
//!
//! sqlparser wraps several nodes in richer types (object name parts that can be
//! functions, limit clauses, `ORDER BY ALL`, insert targets that can be table
//! functions). The analyzer only needs the plain shapes; these helpers give them
//! back in one place.

use sqlsift_ast::{
    Expr, Ident, Insert, LimitClause, ObjectName, ObjectNamePart, OrderBy, OrderByExpr,
    OrderByKind, Query, TableObject,
};

/// The identifier of an object name part. A function part (Snowflake
/// `IDENTIFIER(...)`) gives its function name.
pub(crate) trait ObjectNamePartExt {
    fn ident(&self) -> &Ident;
}

impl ObjectNamePartExt for ObjectNamePart {
    fn ident(&self) -> &Ident {
        match self {
            ObjectNamePart::Identifier(ident) => ident,
            ObjectNamePart::Function(func) => &func.name,
        }
    }
}

/// The identifiers of an object name, cloned.
pub(crate) fn name_idents(name: &ObjectName) -> Vec<Ident> {
    name.0.iter().map(|part| part.ident().clone()).collect()
}

/// The last identifier of an object name (the object itself).
pub(crate) fn name_last(name: &ObjectName) -> Option<&Ident> {
    name.0.last().map(ObjectNamePartExt::ident)
}

/// The column identifier of an INSERT column list entry (sqlparser keeps them as
/// object names; the last part is the column).
pub(crate) fn column_ident(name: &ObjectName) -> &Ident {
    static EMPTY: std::sync::OnceLock<Ident> = std::sync::OnceLock::new();
    name_last(name).unwrap_or_else(|| EMPTY.get_or_init(|| Ident::new("")))
}

/// The expressions of an `ORDER BY` (none for `ORDER BY ALL`).
pub(crate) fn order_by_exprs(order_by: &OrderBy) -> &[OrderByExpr] {
    match &order_by.kind {
        OrderByKind::Expressions(exprs) => exprs,
        OrderByKind::All(_) => &[],
    }
}

/// The `LIMIT` expression of a query.
pub(crate) fn query_limit(query: &Query) -> Option<&Expr> {
    match query.limit_clause.as_ref()? {
        LimitClause::LimitOffset { limit, .. } => limit.as_ref(),
        LimitClause::OffsetCommaLimit { limit, .. } => Some(limit),
    }
}

/// The `OFFSET` expression of a query.
pub(crate) fn query_offset(query: &Query) -> Option<&Expr> {
    match query.limit_clause.as_ref()? {
        LimitClause::LimitOffset { offset, .. } => offset.as_ref().map(|o| &o.value),
        LimitClause::OffsetCommaLimit { offset, .. } => Some(offset),
    }
}

/// The table an INSERT writes to (`None` for a table function or subquery target).
pub(crate) fn insert_table(insert: &Insert) -> Option<&ObjectName> {
    match &insert.table {
        TableObject::TableName(name) => Some(name),
        TableObject::TableFunction(_) | TableObject::TableQuery(_) => None,
    }
}
