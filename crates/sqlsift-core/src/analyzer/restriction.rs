//! Restriction rules (opt-in, off by default): constructs some codebases ban
//!
//! - E0024 unfiltered-write: `UPDATE` / `DELETE` without `WHERE`
//! - E0025 insert-without-columns: `INSERT INTO t VALUES (...)` / `SELECT` without
//!   a column list, which breaks (or silently shifts values) when a column is added
//! - E0026 select-star: `SELECT *` / `t.*` in a statement's result, which changes
//!   when a column is added
//! - E0028 limit-without-order-by: `LIMIT` / `OFFSET` / `FETCH FIRST` without
//!   `ORDER BY`, which returns whichever rows the database comes to first

use std::ops::ControlFlow;

use sqlsift_ast::{
    Delete, Expr, FromTable, ObjectName, Query, SelectItem, SetExpr, Spanned, Statement,
    TableFactor, TableWithJoins, Visit, Visitor,
};

use crate::ast_ext::{insert_table, name_last, query_limit, query_offset, ObjectNamePartExt};
use crate::error::{Diagnostic, DiagnosticKind, Span};

use super::resolver::Resolver;

impl Resolver<'_> {
    pub(super) fn check_restrictions(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Update(update) if update.selection.is_none() => {
                let span = table_span(std::slice::from_ref(&update.table));
                self.diagnostics.push(unfiltered_update(span));
            }
            Statement::Delete(delete) if delete.selection.is_none() && delete.limit.is_none() => {
                self.diagnostics
                    .push(unfiltered_delete(delete_span(delete)));
            }
            Statement::Insert(insert) => {
                if let (true, Some(source)) = (insert.columns.is_empty(), &insert.source) {
                    // `INSERT INTO t DEFAULT VALUES` has no source
                    let span = insert_table(insert)
                        .and_then(name_last)
                        .map(|i| Span::from_sqlparser(&i.span));
                    self.diagnostics.push(insert_without_columns(span));
                    self.check_select_star(source);
                } else if let Some(source) = &insert.source {
                    self.check_select_star(source);
                }
            }
            Statement::Query(query) => self.check_select_star(query),
            _ => {}
        }
    }

    /// E0028 for the queries of a statement, subqueries and CTEs too. Run once, for
    /// the whole statement ([`Self::check_restrictions`] runs again for the INSERT /
    /// UPDATE / DELETE statements in it: `WITH ... INSERT`, data-modifying CTEs)
    pub(super) fn check_unordered_limits(&mut self, stmt: &Statement) {
        let mut unordered = UnorderedLimits::default();
        let _ = stmt.visit(&mut unordered);
        for (keyword, span) in unordered.found {
            self.diagnostics.push(limit_without_order_by(
                keyword,
                (span.line > 0).then_some(span),
            ));
        }
    }

    /// E0026 for wildcards in the result columns of a query
    fn check_select_star(&mut self, query: &Query) {
        let mut stack = vec![query.body.as_ref()];
        while let Some(body) = stack.pop() {
            match body {
                SetExpr::Select(select) => {
                    for item in &select.projection {
                        let (SelectItem::Wildcard(options)
                        | SelectItem::QualifiedWildcard(_, options)) = item
                        else {
                            continue;
                        };
                        let span = Span::from_sqlparser(&options.wildcard_token.0.span);
                        self.diagnostics
                            .push(select_star((span.line > 0).then_some(span)));
                    }
                }
                SetExpr::SetOperation { left, right, .. } => {
                    stack.push(right);
                    stack.push(left);
                }
                SetExpr::Query(inner) => stack.push(inner.body.as_ref()),
                _ => {}
            }
        }
    }
}

fn restriction(kind: DiagnosticKind, message: &str, help: &str, span: Option<Span>) -> Diagnostic {
    let mut diag = Diagnostic::warning(kind, message).with_help(help);
    diag.span = span;
    diag
}

/// E0024 for an `UPDATE` (`span`: the table name)
pub(super) fn unfiltered_update(span: Option<Span>) -> Diagnostic {
    restriction(
        DiagnosticKind::UnfilteredWrite,
        "UPDATE without WHERE changes every row of the table",
        "Add a WHERE clause (WHERE true if every row is meant)",
        span,
    )
}

/// E0024 for a `DELETE` (`span`: the table name)
pub(super) fn unfiltered_delete(span: Option<Span>) -> Diagnostic {
    restriction(
        DiagnosticKind::UnfilteredWrite,
        "DELETE without WHERE deletes every row of the table",
        "Add a WHERE clause (WHERE true if every row is meant), or use TRUNCATE",
        span,
    )
}

/// E0025 (`span`: the table name)
pub(super) fn insert_without_columns(span: Option<Span>) -> Diagnostic {
    restriction(
        DiagnosticKind::InsertWithoutColumns,
        "INSERT without a column list depends on the order of the table's columns",
        "List the target columns: INSERT INTO t (a, b) ...",
        span,
    )
}

/// E0026 (`span`: the `*`)
pub(super) fn select_star(span: Option<Span>) -> Diagnostic {
    restriction(
        DiagnosticKind::SelectStar,
        "SELECT * returns whatever columns the tables have",
        "List the columns the query needs",
        span,
    )
}

/// E0028 (`span`: the value of the limiting clause)
pub(super) fn limit_without_order_by(keyword: &str, span: Option<Span>) -> Diagnostic {
    restriction(
        DiagnosticKind::LimitWithoutOrderBy,
        &format!("{keyword} without ORDER BY returns an arbitrary set of rows"),
        "Add an ORDER BY clause that puts the rows in a fixed order",
        span,
    )
}

/// Queries that limit their rows without ordering them: the limiting keyword and
/// the location of its value
#[derive(Default)]
struct UnorderedLimits {
    found: Vec<(&'static str, Span)>,
    /// The next query is the one of an `EXISTS (...)`, whose rows don't matter
    in_exists: bool,
}

impl Visitor for UnorderedLimits {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        self.in_exists = matches!(expr, Expr::Exists { .. });
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if std::mem::take(&mut self.in_exists) {
            // `EXISTS ((SELECT ...))`: the query in the parentheses is the EXISTS one
            self.in_exists = matches!(query.body.as_ref(), SetExpr::Query(_));
            return ControlFlow::Continue(());
        }
        let (keyword, span) = if is_ordered(query) {
            return ControlFlow::Continue(());
        } else if let Some(limit) = query_limit(query) {
            ("LIMIT", limit.span())
        } else if let Some(offset) = query_offset(query) {
            ("OFFSET", offset.span())
        } else if let Some(fetch) = &query.fetch {
            // `FETCH FIRST ROW ONLY` has no value to point at
            let value = fetch.quantity.as_ref().map(Spanned::span);
            ("FETCH FIRST", value.unwrap_or_else(|| query.span()))
        } else {
            return ControlFlow::Continue(());
        };
        self.found.push((keyword, Span::from_sqlparser(&span)));
        ControlFlow::Continue(())
    }
}

/// Whether `query` has an `ORDER BY`, its own or that of the parenthesized query it
/// limits: `(SELECT ... ORDER BY id) LIMIT 5`
fn is_ordered(query: &Query) -> bool {
    query.order_by.is_some()
        || matches!(query.body.as_ref(), SetExpr::Query(inner) if is_ordered(inner))
}

fn table_span(tables: &[TableWithJoins]) -> Option<Span> {
    match &tables.first()?.relation {
        TableFactor::Table { name, .. } => name_span(name),
        _ => None,
    }
}

fn name_span(name: &ObjectName) -> Option<Span> {
    name.0.last().map(|i| Span::from_sqlparser(&i.ident().span))
}

fn delete_span(delete: &Delete) -> Option<Span> {
    match &delete.from {
        FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => {
            table_span(tables)
        }
    }
    .or_else(|| delete.tables.first().and_then(name_span))
}
