//! E0031 join-fan-out: an aggregate over a table joined one-to-many, next to
//! another one-to-many join that multiplies its rows
//!
//! ```sql
//! -- orders 1:N order_items, orders 1:N payments
//! SELECT o.id, sum(i.price) FROM orders o
//! JOIN order_items i ON i.order_id = o.id
//! JOIN payments p ON p.order_id = o.id
//! GROUP BY o.id;   -- sum(i.price) counts each item once per payment
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use sqlsift_ast::{
    BinaryOperator, DuplicateTreatment, Expr, Function, FunctionArg, FunctionArgExpr,
    FunctionArguments, Select, SelectItem, Spanned, TableFactor, TableWithJoins, Visit, Visitor,
};

use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::TableDef;

use super::resolver::Resolver;
use super::suspicious::join_condition;

/// A join between two relations of the FROM clause on column equalities
struct Edge {
    /// Relation keys (aliases or table names), lowercase
    left: String,
    right: String,
    /// The joined columns of each side, lowercase
    left_columns: BTreeSet<String>,
    right_columns: BTreeSet<String>,
}

impl Resolver<'_> {
    /// Report aggregates over a relation whose rows another one-to-many join of
    /// the same parent multiplies. Run while the SELECT's scope is current.
    pub(super) fn check_join_fan_out(&mut self, select: &Select) {
        if self.templated_statement || self.catalog.unknown_unique_keys {
            return;
        }
        let aggregates = aggregates(select);
        if aggregates.is_empty() {
            return;
        }
        let Some(frame) = self.scope.current_ref() else {
            return;
        };
        // Catalog tables of the FROM clause by relation key
        let mut tables: BTreeMap<String, &TableDef> = BTreeMap::new();
        for (key, relation) in &frame.relations {
            let table = relation
                .source
                .as_ref()
                .and_then(|source| self.catalog.get_table(source));
            if let Some(table) = table.filter(|t| !t.unknown_unique_keys) {
                tables.insert(key.to_lowercase(), table);
            }
        }
        let Some(edges) = join_edges(&select.from) else {
            return;
        };
        // Relations joined one-to-many to each parent: (child, parent)
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &edges {
            let (Some(left), Some(right)) = (tables.get(&edge.left), tables.get(&edge.right))
            else {
                continue;
            };
            let left_unique = is_unique(left, &edge.left_columns);
            let right_unique = is_unique(right, &edge.right_columns);
            if left_unique && !right_unique {
                children.entry(&edge.left).or_default().push(&edge.right);
            } else if right_unique && !left_unique {
                children.entry(&edge.right).or_default().push(&edge.left);
            }
        }

        let mut found = Vec::new();
        for (function, relation) in &aggregates {
            for (parent, kids) in &children {
                if !kids.iter().any(|k| k == relation) {
                    continue;
                }
                let Some(other) = kids.iter().find(|k| *k != relation) else {
                    continue;
                };
                let (name, alias, other_name, parent_name) = (
                    function.name.to_string(),
                    relation.as_str(),
                    *other,
                    *parent,
                );
                let mut diag = Diagnostic::warning(
                    DiagnosticKind::JoinFanOut,
                    format!(
                        "{name}() over '{alias}' counts each of its rows once per matching row of \
                         '{other_name}': both are joined one-to-many to '{parent_name}'"
                    ),
                )
                .with_help(format!(
                    "Aggregate '{alias}' in a subquery or LATERAL join grouped by its join \
                     column, then join the result to '{parent_name}'"
                ));
                let span = function.span();
                if span.start.line > 0 {
                    diag.span = Some(Span::from_sqlparser(&span));
                }
                found.push(diag);
                break;
            }
        }
        self.diagnostics.extend(found);
    }
}

/// Whether the columns cover a primary or unique key of the table
fn is_unique(table: &TableDef, columns: &BTreeSet<String>) -> bool {
    let covers =
        |key: &[String]| !key.is_empty() && key.iter().all(|c| columns.contains(&c.to_lowercase()));
    table
        .primary_key
        .as_ref()
        .is_some_and(|pk| covers(&pk.columns))
        || table
            .columns
            .values()
            .any(|c| c.is_primary_key && columns.contains(&c.name.to_lowercase()))
        || table.unique_constraints.iter().any(|u| covers(&u.columns))
}

/// The joins of the FROM clause as edges, or `None` when a join condition is
/// anything but column equalities between two relations (a filter may make a
/// side unique)
fn join_edges(from: &[TableWithJoins]) -> Option<Vec<Edge>> {
    let mut edges: Vec<Edge> = Vec::new();
    for item in from {
        if !matches!(item.relation, TableFactor::Table { .. }) {
            return None;
        }
        for join in &item.joins {
            if !matches!(join.relation, TableFactor::Table { .. }) {
                return None;
            }
            let on = join_condition(&join.join_operator)?;
            for conjunct in conjuncts(on) {
                let Expr::BinaryOp {
                    left,
                    op: BinaryOperator::Eq,
                    right,
                } = conjunct
                else {
                    return None;
                };
                let (Some((lk, lc)), Some((rk, rc))) = (qualified(left), qualified(right)) else {
                    return None;
                };
                if lk == rk {
                    return None;
                }
                let (lk, lc, rk, rc) = if lk < rk {
                    (lk, lc, rk, rc)
                } else {
                    (rk, rc, lk, lc)
                };
                match edges.iter_mut().find(|e| e.left == lk && e.right == rk) {
                    Some(edge) => {
                        edge.left_columns.insert(lc);
                        edge.right_columns.insert(rc);
                    }
                    None => edges.push(Edge {
                        left: lk,
                        right: rk,
                        left_columns: BTreeSet::from([lc]),
                        right_columns: BTreeSet::from([rc]),
                    }),
                }
            }
        }
    }
    Some(edges)
}

/// `relation.column` (lowercase)
fn qualified(expr: &Expr) -> Option<(String, String)> {
    match expr {
        Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
            Some((parts[0].value.to_lowercase(), parts[1].value.to_lowercase()))
        }
        Expr::Nested(inner) => qualified(inner),
        _ => None,
    }
}

fn conjuncts(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let mut out = conjuncts(left);
            out.extend(conjuncts(right));
            out
        }
        Expr::Nested(inner) => conjuncts(inner),
        other => vec![other],
    }
}

/// `sum`, `avg` and `count` calls of the select list and HAVING (not DISTINCT,
/// not window functions) whose argument references the columns of exactly one
/// relation, with that relation's key (lowercase)
fn aggregates(select: &Select) -> Vec<(&Function, String)> {
    struct Collector<'s>(Vec<&'s Function>);
    impl<'s> Collector<'s> {
        fn collect(&mut self, expr: &'s Expr) {
            let mut stack = vec![expr];
            while let Some(e) = stack.pop() {
                match e {
                    Expr::Function(f) => self.0.push(f),
                    Expr::BinaryOp { left, right, .. } => {
                        stack.push(left);
                        stack.push(right);
                    }
                    Expr::Nested(inner)
                    | Expr::Cast { expr: inner, .. }
                    | Expr::UnaryOp { expr: inner, .. } => stack.push(inner),
                    _ => {}
                }
            }
        }
    }
    let mut collector = Collector(Vec::new());
    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                collector.collect(e);
            }
            _ => {}
        }
    }
    if let Some(having) = &select.having {
        collector.collect(having);
    }
    collector
        .0
        .into_iter()
        .filter_map(|function| {
            let name = function.name.to_string().to_lowercase();
            if !matches!(name.as_str(), "sum" | "avg" | "count") || function.over.is_some() {
                return None;
            }
            let FunctionArguments::List(list) = &function.args else {
                return None;
            };
            if matches!(list.duplicate_treatment, Some(DuplicateTreatment::Distinct)) {
                return None;
            }
            let [FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))] = list.args.as_slice() else {
                return None;
            };
            let mut relations = Relations::default();
            let _ = arg.visit(&mut relations);
            let mut keys = relations.0.into_iter();
            let key = keys.next()?;
            (keys.next().is_none() && !relations.1).then_some((function, key))
        })
        .collect()
}

/// The relation keys an expression's qualified columns name; `.1` when it has
/// an unqualified column or a subquery
#[derive(Default)]
struct Relations(BTreeSet<String>, bool);

impl Visitor for Relations {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::CompoundIdentifier(parts) if parts.len() == 2 => {
                self.0.insert(parts[0].value.to_lowercase());
            }
            Expr::Identifier(_)
            | Expr::CompoundIdentifier(_)
            | Expr::Subquery(_)
            | Expr::Exists { .. }
            | Expr::InSubquery { .. } => self.1 = true,
            _ => {}
        }
        ControlFlow::Continue(())
    }
}
