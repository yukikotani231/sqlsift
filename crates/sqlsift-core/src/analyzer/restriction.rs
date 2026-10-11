//! Restriction rules (opt-in, off by default): constructs some codebases ban
//!
//! - E0024 unfiltered-write: `UPDATE` / `DELETE` without `WHERE`
//! - E0025 insert-without-columns: `INSERT INTO t VALUES (...)` / `SELECT` without
//!   a column list, which breaks (or silently shifts values) when a column is added
//! - E0026 select-star: `SELECT *` / `t.*` in a statement's result, which changes
//!   when a column is added

use sqlparser::ast::{
    Delete, FromTable, ObjectName, Query, SelectItem, SetExpr, Statement, TableFactor,
    TableWithJoins,
};

use crate::ast_ext::{insert_table, name_last, ObjectNamePartExt};
use crate::error::{Diagnostic, DiagnosticKind, Span};

use super::resolver::Resolver;

impl Resolver<'_> {
    pub(super) fn check_restrictions(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Update(update) if update.selection.is_none() => {
                let span = table_span(std::slice::from_ref(&update.table));
                self.push_restriction(
                    DiagnosticKind::UnfilteredWrite,
                    "UPDATE without WHERE changes every row of the table",
                    "Add a WHERE clause (WHERE true if every row is meant)",
                    span,
                );
            }
            Statement::Delete(delete) if delete.selection.is_none() && delete.limit.is_none() => {
                self.push_restriction(
                    DiagnosticKind::UnfilteredWrite,
                    "DELETE without WHERE deletes every row of the table",
                    "Add a WHERE clause (WHERE true if every row is meant), or use TRUNCATE",
                    delete_span(delete),
                );
            }
            Statement::Insert(insert) => {
                if let (true, Some(source)) = (insert.columns.is_empty(), &insert.source) {
                    // `INSERT INTO t DEFAULT VALUES` has no source
                    let span = insert_table(insert)
                        .and_then(name_last)
                        .map(|i| Span::from_sqlparser(&i.span));
                    self.push_restriction(
                        DiagnosticKind::InsertWithoutColumns,
                        "INSERT without a column list depends on the order of the table's columns",
                        "List the target columns: INSERT INTO t (a, b) ...",
                        span,
                    );
                    self.check_select_star(source);
                } else if let Some(source) = &insert.source {
                    self.check_select_star(source);
                }
            }
            Statement::Query(query) => self.check_select_star(query),
            _ => {}
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
                        self.push_restriction(
                            DiagnosticKind::SelectStar,
                            "SELECT * returns whatever columns the tables have",
                            "List the columns the query needs",
                            (span.line > 0).then_some(span),
                        );
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

    fn push_restriction(
        &mut self,
        kind: DiagnosticKind,
        message: &str,
        help: &str,
        span: Option<Span>,
    ) {
        let mut diag = Diagnostic::warning(kind, message).with_help(help);
        diag.span = span;
        self.diagnostics.push(diag);
    }
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
