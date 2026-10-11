//! Suspicious queries: valid SQL that almost certainly doesn't do what was meant
//!
//! - E0018 null-comparison: `x = NULL`, `x <> NULL`, `CASE x WHEN NULL`
//! - E0019 not-in-with-nulls: `x NOT IN (SELECT c ...)` over a nullable column `c`
//! - E0020 outer-column-in-subquery: `x IN (SELECT c FROM t)` where `t` has no column
//!   `c`, so `c` is the outer query's column
//! - E0021 missing-join-condition: `FROM a, b` with nothing that links `a` and `b`
//! - E0022 constant-condition: `id IS NULL` on a NOT NULL column, `a.x = a.x`
//! - E0023 outer-join-filtered: `a LEFT JOIN b ... WHERE b.x = 1`, an inner join
//!
//! These are reported as warnings by default (category `suspicious`).

use std::collections::HashSet;
use std::ops::ControlFlow;

use sqlparser::ast::{
    BinaryOperator, Expr, GroupByExpr, Ident, JoinConstraint, JoinOperator, Select, SelectItem,
    Spanned, TableFactor, TableWithJoins, Value, Visit, Visitor,
};

use crate::error::{Diagnostic, DiagnosticKind, Span};

use super::resolver::{is_comparison_operator, Resolver};
use super::scope::{lookup_ignore_case, ColumnMatch};
use super::structure::visit_flat;

/// How the subquery of an `IN` is used, for the checks of its SELECT
#[derive(Debug, Clone, Copy)]
pub(super) struct InSubquery {
    /// Address of the subquery's SELECT
    pub(super) select: usize,
    /// `NOT IN`
    pub(super) negated: bool,
}

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // E0018 null-comparison
    // ---------------------------------------------------------------------

    /// Report a comparison operator with a NULL literal operand
    pub(super) fn check_null_comparison(&mut self, left: &Expr, op: &BinaryOperator, right: &Expr) {
        if !is_comparison_operator(op) {
            return;
        }
        let other = match (is_null(left), is_null(right)) {
            (false, true) => left,
            (true, false) => right,
            _ => return,
        };
        let help = match op {
            BinaryOperator::Eq => "Use IS NULL to test for NULL",
            BinaryOperator::NotEq => "Use IS NOT NULL to test for a value",
            _ => "Any comparison with NULL is NULL: use IS NULL / IS NOT NULL, or a value",
        };
        let diag = Diagnostic::warning(
            DiagnosticKind::NullComparison,
            format!("Comparison with NULL using '{op}' is never true"),
        )
        .with_help(help);
        self.push_at(diag, other);
    }

    /// Report `CASE x WHEN NULL`, which never matches
    pub(super) fn check_case_when_null(&mut self, operand: &Expr, conditions: &[Expr]) {
        if conditions.iter().any(is_null) {
            let diag = Diagnostic::warning(
                DiagnosticKind::NullComparison,
                "CASE ... WHEN NULL never matches: it compares with '='",
            )
            .with_help("Use CASE WHEN x IS NULL THEN ...");
            self.push_at(diag, operand);
        }
    }

    // ---------------------------------------------------------------------
    // E0019 not-in-with-nulls, E0020 outer-column-in-subquery
    // ---------------------------------------------------------------------

    /// Check the SELECT of an `IN` / `NOT IN` subquery, while its scope is current
    pub(super) fn check_in_subquery_select(&mut self, select: &Select, role: InSubquery) {
        let [item] = select.projection.as_slice() else {
            return;
        };
        let (SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. }) = item else {
            return;
        };
        if let Expr::Identifier(ident) = expr {
            if self.report_outer_column(ident) {
                return;
            }
        }
        if role.negated {
            self.check_not_in_nullable(select, expr);
        }
    }

    /// E0020 for an unqualified column the subquery's own relations don't have
    fn report_outer_column(&mut self, ident: &Ident) -> bool {
        let name = &ident.value;
        let mut frames = self.scope.visible();
        let Some(current) = frames.next() else {
            return false;
        };
        if current.relations.is_empty()
            || current
                .relations
                .values()
                .any(|r| !matches!(r.column(name, self.dialect), ColumnMatch::No))
        {
            return false;
        }
        let Some(outer) = frames.find_map(|frame| {
            frame
                .relations
                .values()
                .find(|r| matches!(r.column(name, self.dialect), ColumnMatch::Yes(_)))
        }) else {
            return false;
        };
        let inner: Vec<&str> = current
            .relations
            .values()
            .map(|r| r.name.as_str())
            .collect();
        let message = format!(
            "Column '{name}' in the subquery is the outer query's '{}.{name}': {} has no column '{name}'",
            outer.name,
            inner.join(", "),
        );
        let help = format!(
            "The subquery then returns the outer row's own value. Select a column of {}, \
             or qualify it ({}.{name}) if this is intended",
            inner.join(", "),
            outer.name,
        );
        self.diagnostics.push(
            Diagnostic::warning(DiagnosticKind::OuterColumnInSubquery, message)
                .with_span(Span::from_sqlparser(&ident.span))
                .with_help(help),
        );
        true
    }

    /// E0019 for `NOT IN (SELECT c FROM t)` where `t.c` is nullable and nothing in
    /// the subquery filters out NULLs
    fn check_not_in_nullable(&mut self, select: &Select, expr: &Expr) {
        let (qualifier, column) = match expr {
            Expr::Identifier(ident) => (None, ident),
            Expr::CompoundIdentifier(idents) if idents.len() == 2 => (Some(&idents[0]), &idents[1]),
            _ => return,
        };
        let Some(frame) = self.scope.current_ref() else {
            return;
        };
        // The relation the column belongs to, by its key in FROM
        let key = if let Some(q) = qualifier {
            q.value.to_lowercase()
        } else {
            let mut owners = frame
                .relations
                .iter()
                .filter(|(_, r)| !matches!(r.column(&column.value, self.dialect), ColumnMatch::No));
            let Some((key, _)) = owners.next() else {
                return;
            };
            if owners.next().is_some() {
                return;
            }
            key.to_lowercase()
        };
        let tables = self.catalog_tables_in(&select.from);
        let Some((_, name)) = tables.iter().find(|(k, _)| *k == key) else {
            return;
        };
        let Some(table) = self.catalog.get_table(name) else {
            return;
        };
        let Some(def) = table.get_column(&column.value) else {
            return;
        };
        let in_primary_key = def.is_primary_key
            || table
                .primary_key
                .as_ref()
                .is_some_and(|pk| pk.columns.iter().any(|c| c.eq_ignore_ascii_case(&def.name)));
        if !def.nullable || in_primary_key || filters_column(select, &column.value) {
            return;
        }
        let message = format!(
            "NOT IN over the nullable column '{}.{}' returns no rows if it has a NULL",
            table.name.name, def.name
        );
        let help = format!(
            "Use NOT EXISTS, or add WHERE {} IS NOT NULL to the subquery",
            column.value
        );
        let span = Span::from_sqlparser(&expr.span());
        self.diagnostics.push(
            Diagnostic::warning(DiagnosticKind::NotInWithNulls, message)
                .with_span(span)
                .with_help(help),
        );
    }

    // ---------------------------------------------------------------------
    // E0021 missing-join-condition
    // ---------------------------------------------------------------------

    /// Report a table of a comma-separated FROM list that no condition links with
    /// the others. Run while the SELECT's scope is current.
    pub(super) fn check_join_conditions(&mut self, select: &Select) {
        if select.from.len() < 2 || self.templated_statement {
            return;
        }
        let Some(frame) = self.scope.current_ref() else {
            return;
        };
        // Relation keys of each comma-separated item (explicit joins are one item)
        let items: Vec<Vec<String>> = select.from.iter().map(item_keys).collect();
        let item_of = |key: &str| {
            items
                .iter()
                .position(|keys| keys.iter().any(|k| k.eq_ignore_ascii_case(key)))
        };
        let mut groups = UnionFind::new(items.len());

        // Items a set of column references belongs to: a qualified reference to
        // its relation, an unqualified one to every item that may have the column
        let referenced = |refs: &[Vec<String>]| -> HashSet<usize> {
            let mut out = HashSet::new();
            for parts in refs {
                match parts.as_slice() {
                    [] => {}
                    [column] => {
                        let owners: Vec<usize> = (0..items.len())
                            .filter(|&i| {
                                items[i].iter().any(|k| {
                                    lookup_ignore_case(&frame.relations, k).map_or(true, |r| {
                                        !matches!(r.column(column, self.dialect), ColumnMatch::No)
                                    })
                                })
                            })
                            .collect();
                        // A column no item has (misspelled, or an alias or outer
                        // column) may be meant for any of them
                        if owners.is_empty() {
                            out.extend(0..items.len());
                        }
                        out.extend(owners);
                    }
                    [.., qualifier, _] => {
                        out.extend(item_of(qualifier));
                    }
                }
            }
            out
        };

        let mut link = |refs: &[Vec<String>], extra: Option<usize>| {
            let mut linked: Vec<usize> = referenced(refs).into_iter().collect();
            linked.extend(extra);
            for pair in linked.windows(2) {
                groups.union(pair[0], pair[1]);
            }
            linked.len()
        };

        // Each AND-ed condition of WHERE links the items it references. A
        // condition on a single item (`WHERE t.name = 'rust'`) narrows it down:
        // combining every row with a few chosen ones is a deliberate cross join.
        if let Some(selection) = &select.selection {
            for conjunct in conjuncts(selection) {
                if link(&references(conjunct), None) == 1 {
                    return;
                }
            }
        }
        for (i, item) in select.from.iter().enumerate() {
            // ON conditions, and (LATERAL) subqueries, table functions and BigQuery
            // array paths that refer to earlier items
            for factor in
                std::iter::once(&item.relation).chain(item.joins.iter().map(|j| &j.relation))
            {
                link(&factor_references(factor), Some(i));
            }
            for join in &item.joins {
                if let Some(on) = join_condition(&join.join_operator) {
                    link(&references(on), Some(i));
                }
            }
        }

        // Only plain tables count: a CTE, subquery or function may return one row
        let table_item = |i: usize| match &select.from[i].relation {
            TableFactor::Table {
                name, args: None, ..
            } if select.from[i].joins.is_empty() => {
                let qualified = self.catalog.qualified_name(name);
                let cte = qualified.schema.is_none()
                    && self.scope.cte(self.catalog, &qualified.name).is_some();
                (!cte && self.catalog.get_table(&qualified).is_some()).then_some(name)
            }
            _ => None,
        };
        let first_group = (0..items.len())
            .find(|&i| table_item(i).is_some())
            .map(|i| groups.find(i));
        let Some(first_group) = first_group else {
            return;
        };
        let mut reported = HashSet::new();
        for (i, keys) in items.iter().enumerate() {
            let group = groups.find(i);
            if group == first_group || reported.contains(&group) {
                continue;
            }
            let Some(name) = table_item(i) else {
                continue;
            };
            reported.insert(group);
            let key = keys.first().cloned().unwrap_or_default();
            let span = name.0.last().map(|ident| Span::from_sqlparser(&ident.span));
            let mut diag = Diagnostic::warning(
                DiagnosticKind::MissingJoinCondition,
                format!(
                    "No condition links '{key}' with the other tables: every row is joined \
                     with every row (cross join)"
                ),
            )
            .with_help(format!(
                "Add a join condition for '{key}' to WHERE, or write CROSS JOIN if every \
                 combination is intended"
            ));
            diag.span = span;
            self.diagnostics.push(diag);
        }
    }

    // ---------------------------------------------------------------------
    // E0022 constant-condition, E0023 outer-join-filtered
    // ---------------------------------------------------------------------

    /// Report a column compared with itself (`a.x = a.x`)
    pub(super) fn check_self_comparison(&mut self, left: &Expr, op: &BinaryOperator, right: &Expr) {
        if !is_comparison_operator(op) {
            return;
        }
        let (Some(l), Some(r)) = (column_parts(left), column_parts(right)) else {
            return;
        };
        if l.len() != r.len() || !l.iter().zip(&r).all(|(a, b)| same_ident(a, b)) {
            return;
        }
        let outcome = match op {
            BinaryOperator::Eq | BinaryOperator::LtEq | BinaryOperator::GtEq => {
                "true for every non-NULL value"
            }
            _ => "never true",
        };
        let column = l
            .iter()
            .map(|i| i.value.as_str())
            .collect::<Vec<_>>()
            .join(".");
        let diag = Diagnostic::warning(
            DiagnosticKind::ConstantCondition,
            format!("'{column} {op} {column}' compares a column with itself: it is {outcome}"),
        )
        .with_help("Compare with the column of the other table, or remove the condition");
        self.push_at(diag, left);
    }

    /// Check WHERE of a SELECT for `IS NULL` tests of NOT NULL columns and
    /// conditions that undo an outer join. Run while the SELECT's scope is current.
    pub(super) fn check_where_conditions(&mut self, select: &Select) {
        let Some(selection) = &select.selection else {
            return;
        };
        if self.templated_statement {
            return;
        }
        let mut nullable = HashSet::new();
        for item in &select.from {
            outer_join_keys(item, &mut nullable);
        }
        let tables = self.catalog_tables_in(&select.from);
        let mut found = Vec::new();

        // `col IS NULL` anywhere in AND / OR / NOT
        let mut stack = vec![selection];
        while let Some(expr) = stack.pop() {
            match expr {
                Expr::BinaryOp {
                    left,
                    op: BinaryOperator::And | BinaryOperator::Or,
                    right,
                } => {
                    stack.push(right);
                    stack.push(left);
                }
                Expr::UnaryOp { expr, .. } | Expr::Nested(expr) => stack.push(expr),
                Expr::IsNull(inner) => {
                    let Some((key, column)) = self.frame_column(inner) else {
                        continue;
                    };
                    if nullable.contains(&key) {
                        continue;
                    }
                    let Some((_, name)) = tables.iter().find(|(k, _)| *k == key) else {
                        continue;
                    };
                    let Some(def) = self
                        .catalog
                        .get_table(name)
                        .and_then(|t| t.get_column(&column))
                    else {
                        continue;
                    };
                    if !def.nullable {
                        found.push((
                            Diagnostic::warning(
                                DiagnosticKind::ConstantCondition,
                                format!(
                                    "Column '{}.{}' is NOT NULL: IS NULL is never true",
                                    name.name, def.name
                                ),
                            )
                            .with_help("Remove the condition, or test a nullable column"),
                            inner.as_ref(),
                        ));
                    }
                }
                _ => {}
            }
        }

        // A condition on a column of the optional side of an outer join, AND-ed
        // into WHERE, removes the rows the join adds without a match
        let mut reported = HashSet::new();
        for conjunct in conjuncts(selection) {
            let Some(column) = null_rejected_column(conjunct) else {
                continue;
            };
            let Some((key, _)) = self.frame_column(column) else {
                continue;
            };
            if !nullable.contains(&key) || !reported.insert(key.clone()) {
                continue;
            }
            let shown = column_parts(column)
                .map(|p| {
                    p.iter()
                        .map(|i| i.value.as_str())
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .unwrap_or_default();
            found.push((
                Diagnostic::warning(
                    DiagnosticKind::OuterJoinFiltered,
                    format!(
                        "WHERE condition on '{shown}' removes the rows the outer join adds \
                         without a match in '{key}': it works as an inner join"
                    ),
                )
                .with_help(format!(
                    "Move the condition into the join's ON clause to keep unmatched rows, \
                     or write an inner JOIN (or test '{shown} IS NULL OR ...')"
                )),
                column,
            ));
        }
        for (diag, at) in found {
            self.push_at(diag, at);
        }
    }

    /// The relation key (lowercase) and column name a column reference resolves to
    /// in the innermost query block, if it resolves there for certain
    fn frame_column(&self, expr: &Expr) -> Option<(String, String)> {
        let parts = column_parts(expr)?;
        let frame = self.scope.current_ref()?;
        match parts.as_slice() {
            [column] => {
                let mut owner = None;
                for (key, relation) in &frame.relations {
                    match relation.column(&column.value, self.dialect) {
                        ColumnMatch::No => {}
                        ColumnMatch::Yes(_) if owner.is_none() => owner = Some(key),
                        _ => return None,
                    }
                }
                Some((owner?.to_lowercase(), column.value.clone()))
            }
            [qualifier, column] => {
                let relation = lookup_ignore_case(&frame.relations, &qualifier.value)?;
                matches!(
                    relation.column(&column.value, self.dialect),
                    ColumnMatch::Yes(_)
                )
                .then(|| (qualifier.value.to_lowercase(), column.value.clone()))
            }
            _ => None,
        }
    }

    fn push_at(&mut self, mut diag: Diagnostic, expr: &Expr) {
        diag.span = super::type_check::located_span(expr);
        self.diagnostics.push(diag);
    }
}

fn is_null(expr: &Expr) -> bool {
    match expr {
        Expr::Value(Value::Null) => true,
        Expr::Nested(inner) => is_null(inner),
        _ => false,
    }
}

/// Whether anything in the SELECT but its select list mentions a column with this
/// name (a condition on it, a join on it or a grouping by it may exclude NULLs)
fn filters_column(select: &Select, column: &str) -> bool {
    if select
        .from
        .iter()
        .flat_map(|t| &t.joins)
        .any(|j| join_uses(&j.join_operator))
    {
        return true;
    }
    let mut exprs: Vec<&Expr> = Vec::new();
    exprs.extend(select.selection.as_ref());
    exprs.extend(select.having.as_ref());
    exprs.extend(select.qualify.as_ref());
    if let GroupByExpr::Expressions(group_by, _) = &select.group_by {
        exprs.extend(group_by);
    }
    for join in select.from.iter().flat_map(|t| &t.joins) {
        exprs.extend(join_condition(&join.join_operator));
    }
    exprs.into_iter().any(|expr| {
        references(expr)
            .iter()
            .any(|parts| parts.last().is_some_and(|c| c.eq_ignore_ascii_case(column)))
    })
}

/// Relation keys of a FROM item and the tables joined to it
fn item_keys(item: &TableWithJoins) -> Vec<String> {
    fn walk(factor: &TableFactor, out: &mut Vec<String>) {
        match factor {
            TableFactor::NestedJoin {
                table_with_joins,
                alias: None,
            } => {
                walk(&table_with_joins.relation, out);
                for join in &table_with_joins.joins {
                    walk(&join.relation, out);
                }
            }
            TableFactor::Table { name, alias, .. } => {
                out.extend(
                    alias
                        .as_ref()
                        .map(|a| a.name.value.clone())
                        .or_else(|| name.0.last().map(|i| i.value.clone())),
                );
            }
            other => {
                if let Some(alias) = factor_alias(other) {
                    out.push(alias.value.clone());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&item.relation, &mut out);
    for join in &item.joins {
        walk(&join.relation, &mut out);
    }
    out
}

fn factor_alias(factor: &TableFactor) -> Option<&Ident> {
    match factor {
        TableFactor::Derived { alias, .. }
        | TableFactor::TableFunction { alias, .. }
        | TableFactor::Function { alias, .. }
        | TableFactor::UNNEST { alias, .. }
        | TableFactor::JsonTable { alias, .. }
        | TableFactor::NestedJoin { alias, .. } => alias.as_ref().map(|a| &a.name),
        _ => None,
    }
}

/// Column references of what a FROM item computes from (a subquery, function
/// arguments, a BigQuery array path `t.items`): they may refer to earlier items
fn factor_references(factor: &TableFactor) -> Vec<Vec<String>> {
    match factor {
        TableFactor::Table { name, args, .. } => {
            let mut refs = Vec::new();
            // `FROM t, t.items`: an array path (a qualifier and a column)
            if name.0.len() >= 2 {
                refs.push(name.0.iter().map(|i| i.value.clone()).collect());
            }
            if let Some(args) = args {
                let mut collector = References::default();
                let _ = args.visit(&mut collector);
                refs.extend(collector.0);
            }
            refs
        }
        TableFactor::NestedJoin {
            table_with_joins, ..
        } => std::iter::once(&table_with_joins.relation)
            .chain(table_with_joins.joins.iter().map(|j| &j.relation))
            .flat_map(factor_references)
            .collect(),
        other => {
            let mut collector = References::default();
            let _ = other.visit(&mut collector);
            collector.0
        }
    }
}

fn join_condition(op: &JoinOperator) -> Option<&Expr> {
    match op {
        JoinOperator::Inner(JoinConstraint::On(e))
        | JoinOperator::LeftOuter(JoinConstraint::On(e))
        | JoinOperator::RightOuter(JoinConstraint::On(e))
        | JoinOperator::FullOuter(JoinConstraint::On(e))
        | JoinOperator::LeftSemi(JoinConstraint::On(e))
        | JoinOperator::RightSemi(JoinConstraint::On(e))
        | JoinOperator::LeftAnti(JoinConstraint::On(e))
        | JoinOperator::RightAnti(JoinConstraint::On(e))
        | JoinOperator::Semi(JoinConstraint::On(e))
        | JoinOperator::Anti(JoinConstraint::On(e))
        | JoinOperator::AsOf {
            constraint: JoinConstraint::On(e),
            ..
        } => Some(e),
        _ => None,
    }
}

/// Whether a join matches columns by name (`USING`, `NATURAL`)
fn join_uses(op: &JoinOperator) -> bool {
    match op {
        JoinOperator::Inner(c)
        | JoinOperator::LeftOuter(c)
        | JoinOperator::RightOuter(c)
        | JoinOperator::FullOuter(c)
        | JoinOperator::LeftSemi(c)
        | JoinOperator::RightSemi(c)
        | JoinOperator::LeftAnti(c)
        | JoinOperator::RightAnti(c)
        | JoinOperator::Semi(c)
        | JoinOperator::Anti(c) => matches!(c, JoinConstraint::Using(_) | JoinConstraint::Natural),
        _ => false,
    }
}

/// The AND-ed conditions of an expression
fn conjuncts(expr: &Expr) -> Vec<&Expr> {
    let mut out = Vec::new();
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match e {
            Expr::BinaryOp {
                left,
                op: BinaryOperator::And,
                right,
            } => {
                stack.push(right);
                stack.push(left);
            }
            Expr::Nested(inner) => stack.push(inner),
            other => out.push(other),
        }
    }
    out
}

/// Column references in an expression (subqueries included), as name parts
fn references(expr: &Expr) -> Vec<Vec<String>> {
    let mut collector = References::default();
    let _ = visit_flat(expr, &mut collector);
    collector.0
}

#[derive(Default)]
struct References(Vec<Vec<String>>);

impl Visitor for References {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::Identifier(ident) => self.0.push(vec![ident.value.clone()]),
            Expr::CompoundIdentifier(idents) => {
                self.0
                    .push(idents.iter().map(|i| i.value.clone()).collect());
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// Disjoint sets of FROM items
struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.0[a] = b;
    }
}

/// The name parts of a column reference
fn column_parts(expr: &Expr) -> Option<Vec<&Ident>> {
    match expr {
        Expr::Identifier(ident) => Some(vec![ident]),
        Expr::CompoundIdentifier(idents) => Some(idents.iter().collect()),
        Expr::Nested(inner) => column_parts(inner),
        _ => None,
    }
}

fn same_ident(a: &Ident, b: &Ident) -> bool {
    if a.quote_style.is_none() && b.quote_style.is_none() {
        a.value.eq_ignore_ascii_case(&b.value)
    } else {
        a.value == b.value
    }
}

/// The column a condition is NULL for when the column is NULL, which then
/// removes the row: `col = x`, `col > x`, `col IN (...)`, `col BETWEEN ...`,
/// `col LIKE ...`
fn null_rejected_column(condition: &Expr) -> Option<&Expr> {
    let column = match condition {
        Expr::BinaryOp { left, op, right } if is_comparison_operator(op) => {
            match (column_parts(left), column_parts(right)) {
                (Some(_), _) => left.as_ref(),
                (None, Some(_)) => right.as_ref(),
                _ => return None,
            }
        }
        Expr::InList { expr, .. }
        | Expr::Between { expr, .. }
        | Expr::Like { expr, .. }
        | Expr::ILike { expr, .. } => expr.as_ref(),
        Expr::Nested(inner) => return null_rejected_column(inner),
        _ => return None,
    };
    column_parts(column).map(|_| column)
}

/// Relation keys (lowercase) on the optional side of an outer join of a FROM item:
/// the right side of LEFT JOIN, the left side of RIGHT JOIN, both of FULL JOIN.
/// Returns all keys of the item.
fn outer_join_keys(item: &TableWithJoins, nullable: &mut HashSet<String>) -> Vec<String> {
    fn factor_keys(factor: &TableFactor, nullable: &mut HashSet<String>) -> Vec<String> {
        match factor {
            TableFactor::NestedJoin {
                table_with_joins,
                alias: None,
            } => outer_join_keys(table_with_joins, nullable),
            TableFactor::Table { name, alias, .. } => alias
                .as_ref()
                .map(|a| a.name.value.clone())
                .or_else(|| name.0.last().map(|i| i.value.clone()))
                .map(|k| vec![k.to_lowercase()])
                .unwrap_or_default(),
            other => factor_alias(other)
                .map(|a| vec![a.value.to_lowercase()])
                .unwrap_or_default(),
        }
    }
    let mut keys = factor_keys(&item.relation, nullable);
    for join in &item.joins {
        let right = factor_keys(&join.relation, nullable);
        match &join.join_operator {
            JoinOperator::LeftOuter(_) => nullable.extend(right.iter().cloned()),
            JoinOperator::RightOuter(_) => nullable.extend(keys.iter().cloned()),
            JoinOperator::FullOuter(_) => {
                nullable.extend(keys.iter().cloned());
                nullable.extend(right.iter().cloned());
            }
            _ => {}
        }
        keys.extend(right);
    }
    keys
}
