//! Grouping check of the statement resolver (E0016): in an aggregate query, a
//! column used outside aggregate functions must be grouped
//!
//! PostgreSQL rules: a column is grouped when GROUP BY lists it, when a GROUP BY
//! expression contains the expression it is used in, or when GROUP BY lists the
//! whole primary key of its table (functional dependency). The check gives up
//! wherever it can't follow these rules exactly: relations with unknown columns,
//! `JOIN ... USING`, GROUPING SETS / ROLLUP / CUBE, statements with template tags,
//! and columns inside calls of functions it doesn't know (which may be
//! user-defined aggregates).

use std::collections::HashSet;
use std::ops::ControlFlow;

use sqlparser::ast::{
    Expr, GroupByExpr, Ident, Query, Select, SelectItem, TableFactor, Value, ValueWithSpan, Visitor,
};

use crate::ast_ext::{order_by_exprs, ObjectNamePartExt};
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};

use super::resolver::Resolver;
use super::scope::{ColumnMatch, Frame};
use super::structure::{is_aggregate, visit_flat};

/// A column of a relation of the query block: (relation key, column), lowercase
type ColumnKey = (String, String);

impl Resolver<'_> {
    /// Report columns of an aggregate query that are neither grouped nor used in an
    /// aggregate function. Runs in the SELECT's frame, after all its clauses.
    pub(super) fn check_grouping(&mut self, select: &Select, query: Option<&Query>) {
        // Template tags can hide GROUP BY columns (`{{ dbt_utils.group_by(3) }}`)
        if !matches!(self.dialect, SqlDialect::PostgreSQL | SqlDialect::Redshift)
            || self.templated_statement
        {
            return;
        }
        let GroupByExpr::Expressions(group_by, modifiers) = &select.group_by else {
            return;
        };
        if !modifiers.is_empty()
            || group_by.iter().any(|e| {
                matches!(
                    e,
                    Expr::GroupingSets(_) | Expr::Cube(_) | Expr::Rollup(_) | Expr::Tuple(_)
                )
            })
        {
            return;
        }
        let order_by: Vec<&Expr> = query
            .and_then(|q| q.order_by.as_ref())
            .map(|o| order_by_exprs(o).iter().map(|e| &e.expr).collect())
            .unwrap_or_default();
        let projection: Vec<&Expr> = select
            .projection
            .iter()
            .filter_map(|item| match item {
                SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => Some(e),
                _ => None,
            })
            .collect();
        let aggregated = !group_by.is_empty()
            || select.having.is_some()
            || projection
                .iter()
                .chain(&order_by)
                .any(|e| has_aggregate(e, self.dialect));
        if !aggregated {
            return;
        }
        // `SELECT *` / `t.*` in an aggregate query: columns aren't listed
        if projection.len() != select.projection.len() {
            return;
        }
        let Some(frame) = self.scope.current_ref() else {
            return;
        };
        if frame.relations.is_empty()
            || frame.relations.values().any(|r| r.columns.is_none())
            || !frame.using_columns.is_empty()
        {
            return;
        }

        // What GROUP BY groups: columns, and other expressions with the column names
        // they use
        let mut grouped: HashSet<ColumnKey> = HashSet::new();
        let mut grouped_exprs: Vec<String> = Vec::new();
        let mut touched: HashSet<String> = HashSet::new();
        for expr in group_by {
            let expr = match expr {
                // A position in the select list
                Expr::Value(ValueWithSpan {
                    value: Value::Number(n, _),
                    ..
                }) => {
                    match n
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| projection.get(i.checked_sub(1)?))
                    {
                        Some(item) => *item,
                        None => return,
                    }
                }
                // An output column alias that isn't an input column
                Expr::Identifier(ident)
                    if self
                        .resolve_column(frame, std::slice::from_ref(ident))
                        .is_none() =>
                {
                    match output_alias(select, &ident.value) {
                        Some(item) => item,
                        None => expr,
                    }
                }
                _ => expr,
            };
            if let Some(column) =
                column_parts(expr).and_then(|parts| self.resolve_column(frame, parts))
            {
                grouped.insert(column);
            } else {
                grouped_exprs.push(normalize(expr));
                let _ = visit_flat(expr, &mut ColumnNames(&mut touched));
            }
        }

        // Tables whose whole primary key is grouped
        let mut whole_tables: HashSet<String> = HashSet::new();
        for (key, table) in self.catalog_tables_in(&select.from) {
            let Some(def) = self.catalog.get_table(&table) else {
                continue;
            };
            let pk: Vec<String> = match &def.primary_key {
                Some(pk) => pk.columns.clone(),
                None => def
                    .columns
                    .values()
                    .filter(|c| c.is_primary_key)
                    .map(|c| c.name.clone())
                    .collect(),
            };
            if !pk.is_empty()
                && pk
                    .iter()
                    .all(|c| grouped.contains(&(key.clone(), c.to_lowercase())))
            {
                whole_tables.insert(key);
            }
        }

        // Column references outside aggregates in the select list, HAVING, ORDER BY
        let output_names: Vec<String> = select
            .projection
            .iter()
            .filter_map(|item| match item {
                SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.to_lowercase()),
                SelectItem::UnnamedExpr(e) => column_parts(e)
                    .and_then(|p| p.last())
                    .map(|i| i.value.to_lowercase()),
                _ => None,
            })
            .collect();
        let mut finder = UngroupedRefs {
            dialect: self.dialect,
            grouped_exprs: &grouped_exprs,
            query_depth: 0,
            skip_depth: 0,
            refs: Vec::new(),
        };
        for expr in &projection {
            let _ = visit_flat(expr, &mut finder);
        }
        if let Some(having) = &select.having {
            let _ = visit_flat(having, &mut finder);
        }
        for expr in order_by {
            // Positions and output column names
            let output = matches!(expr, Expr::Value(_))
                || matches!(expr, Expr::Identifier(i) if output_names.contains(&i.value.to_lowercase()));
            if !output {
                let _ = visit_flat(expr, &mut finder);
            }
        }

        let mut reported: HashSet<ColumnKey> = HashSet::new();
        let mut diagnostics = Vec::new();
        for parts in finder.refs {
            let Some(column) = self.resolve_column(frame, &parts) else {
                continue;
            };
            if grouped.contains(&column)
                || whole_tables.contains(&column.0)
                || touched.contains(&column.1)
                || !reported.insert(column.clone())
            {
                continue;
            }
            let Some(ident) = parts.last() else {
                continue;
            };
            let name = format!("{}.{}", column.0, ident.value);
            diagnostics.push(
                Diagnostic::error(
                    DiagnosticKind::GroupingError,
                    format!(
                        "Column '{name}' must appear in GROUP BY or be used in an aggregate function"
                    ),
                )
                .with_span(Span::from_sqlparser(&ident.span))
                .with_help(format!(
                    "Add '{name}' to GROUP BY, or aggregate it (for example max({name}))"
                )),
            );
        }
        self.diagnostics.extend(diagnostics);
    }

    /// The column of the query block a column reference names, if it is exactly one
    /// column of one relation with known columns
    fn resolve_column(&self, frame: &Frame, parts: &[Ident]) -> Option<ColumnKey> {
        match parts {
            [column] => {
                let mut found = None;
                for (key, relation) in &frame.relations {
                    match relation.column(&column.value, self.dialect) {
                        ColumnMatch::Yes(_) if found.is_none() => {
                            found = Some((key.to_lowercase(), column.value.to_lowercase()));
                        }
                        ColumnMatch::No => {}
                        // Ambiguous, or the relation may have it
                        _ => return None,
                    }
                }
                found
            }
            [table, column] => {
                let (key, relation) = frame
                    .relations
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&table.value))?;
                match relation.column(&column.value, self.dialect) {
                    ColumnMatch::Yes(_) => Some((key.to_lowercase(), column.value.to_lowercase())),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Catalog tables of a FROM clause with the (lowercase) key they are known by
    pub(super) fn catalog_tables_in(
        &self,
        from: &[sqlparser::ast::TableWithJoins],
    ) -> Vec<(String, crate::schema::QualifiedName)> {
        fn walk(
            resolver: &Resolver<'_>,
            factor: &TableFactor,
            out: &mut Vec<(String, crate::schema::QualifiedName)>,
        ) {
            match factor {
                TableFactor::Table {
                    name,
                    alias,
                    args: None,
                    ..
                } => {
                    let key = alias
                        .as_ref()
                        .map(|a| a.name.value.clone())
                        .or_else(|| name.0.last().map(|i| i.ident().value.clone()));
                    if let Some(key) = key {
                        out.push((key.to_lowercase(), resolver.catalog.qualified_name(name)));
                    }
                }
                TableFactor::NestedJoin {
                    table_with_joins,
                    alias: None,
                } => {
                    walk(resolver, &table_with_joins.relation, out);
                    for join in &table_with_joins.joins {
                        walk(resolver, &join.relation, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for table in from {
            walk(self, &table.relation, &mut out);
            for join in &table.joins {
                walk(self, &join.relation, &mut out);
            }
        }
        // A CTE named like a table hides it
        out.retain(|(_, name)| {
            name.schema.is_some() || self.scope.cte(self.catalog, &name.name).is_none()
        });
        out
    }
}

/// The identifiers of a plain column reference (`c`, `t.c`)
fn column_parts(expr: &Expr) -> Option<&[Ident]> {
    match expr {
        Expr::Identifier(ident) => Some(std::slice::from_ref(ident)),
        Expr::CompoundIdentifier(idents) => Some(idents),
        Expr::Nested(inner) => column_parts(inner),
        _ => None,
    }
}

/// The expression of the select item whose alias is `name`
fn output_alias<'s>(select: &'s Select, name: &str) -> Option<&'s Expr> {
    select.projection.iter().find_map(|item| match item {
        SelectItem::ExprWithAlias { expr, alias } if alias.value.eq_ignore_ascii_case(name) => {
            Some(expr)
        }
        _ => None,
    })
}

/// An expression's text for comparing GROUP BY expressions with other clauses
fn normalize(expr: &Expr) -> String {
    expr.to_string().to_lowercase()
}

/// Whether an expression calls an aggregate function (outside subqueries)
fn has_aggregate(expr: &Expr, dialect: SqlDialect) -> bool {
    struct Finder {
        dialect: SqlDialect,
        depth: usize,
    }
    impl Visitor for Finder {
        type Break = ();
        fn pre_visit_query(&mut self, _q: &Query) -> ControlFlow<()> {
            self.depth += 1;
            ControlFlow::Continue(())
        }
        fn post_visit_query(&mut self, _q: &Query) -> ControlFlow<()> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
        fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
            match expr {
                Expr::Function(f)
                    if self.depth == 0 && f.over.is_none() && is_aggregate(f, self.dialect) =>
                {
                    ControlFlow::Break(())
                }
                _ => ControlFlow::Continue(()),
            }
        }
    }
    visit_flat(expr, &mut Finder { dialect, depth: 0 }).is_break()
}

/// Collects the (lowercase) column names an expression uses
struct ColumnNames<'a>(&'a mut HashSet<String>);

impl Visitor for ColumnNames<'_> {
    type Break = ();
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Some(last) = column_parts(expr).and_then(<[Ident]>::last) {
            self.0.insert(last.value.to_lowercase());
        }
        ControlFlow::Continue(())
    }
}

/// Scalar functions whose arguments are checked; arguments of any other function
/// are skipped, since it may be a user-defined aggregate
fn is_known_scalar(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "lower"
            | "upper"
            | "coalesce"
            | "nullif"
            | "concat"
            | "length"
            | "char_length"
            | "round"
            | "abs"
            | "date_trunc"
            | "to_char"
            | "trim"
            | "btrim"
            | "ltrim"
            | "rtrim"
            | "replace"
            | "substr"
            | "left"
            | "right"
            | "greatest"
            | "least"
    )
}

/// Collects column references outside aggregates, subqueries, unknown functions
/// and grouped expressions
struct UngroupedRefs<'a> {
    dialect: SqlDialect,
    grouped_exprs: &'a [String],
    query_depth: usize,
    /// Depth of subtrees whose column references don't need to be grouped
    skip_depth: usize,
    refs: Vec<Vec<Ident>>,
}

impl UngroupedRefs<'_> {
    fn skips(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Function(f) => {
                let known = f.over.is_none()
                    && f.name.0.len() == 1
                    && f.name
                        .0
                        .first()
                        .is_some_and(|n| is_known_scalar(&n.ident().value));
                is_aggregate(f, self.dialect) && f.over.is_none() || !known
            }
            _ => {
                !self.grouped_exprs.is_empty()
                    && column_parts(expr).is_none()
                    && self.grouped_exprs.contains(&normalize(expr))
            }
        }
    }
}

impl Visitor for UngroupedRefs<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, _q: &Query) -> ControlFlow<()> {
        self.query_depth += 1;
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _q: &Query) -> ControlFlow<()> {
        self.query_depth -= 1;
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if self.query_depth > 0 {
            return ControlFlow::Continue(());
        }
        if self.skips(expr) {
            self.skip_depth += 1;
        } else if self.skip_depth == 0 {
            if let Some(parts) = column_parts(expr) {
                if !matches!(expr, Expr::Nested(_)) {
                    self.refs.push(parts.to_vec());
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if self.query_depth == 0 && self.skips(expr) {
            self.skip_depth -= 1;
        }
        ControlFlow::Continue(())
    }
}
