//! Structural checks of the statement resolver: rules about where things may appear
//! and how often, which need the query's shape rather than its types
//!
//! Checks:
//! - a table alias or CTE name given twice in one query block (E0009)
//! - a column given twice in an INSERT column list or UPDATE SET (E0010)
//! - an ORDER BY / GROUP BY position past the end of the select list (E0011)
//! - aggregate and window functions where SQL doesn't allow them (E0012)
//!
//! Each check is limited to the dialects known to reject the query.

use std::ops::ControlFlow;

use sqlparser::ast::{
    Assignment, AssignmentTarget, Expr, Function, FunctionArguments, GroupByExpr, Ident, Query,
    Select, Spanned, TableFactor, TableWithJoins, Value, Visit, Visitor,
};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};

use super::resolver::Resolver;

/// A clause in which aggregate or window functions are not allowed
#[derive(Debug, Clone, Copy)]
pub(super) enum Clause {
    Where,
    JoinOn,
    GroupBy,
    Having,
    UpdateSet,
    Values,
}

impl Clause {
    fn name(self) -> &'static str {
        match self {
            Clause::Where => "WHERE",
            Clause::JoinOn => "JOIN conditions",
            Clause::GroupBy => "GROUP BY",
            Clause::Having => "HAVING",
            Clause::UpdateSet => "UPDATE SET",
            Clause::Values => "VALUES",
        }
    }

    /// Whether aggregate functions are allowed (only HAVING of these)
    fn allows_aggregates(self) -> bool {
        matches!(self, Clause::Having)
    }

    /// Where to use the function instead
    fn aggregate_help(self) -> &'static str {
        match self {
            Clause::Where | Clause::JoinOn => {
                "Filter on aggregates in HAVING, or aggregate in a subquery"
            }
            Clause::GroupBy => "Group by the aggregated expression's input, not the aggregate",
            _ => "Compute the aggregate in a subquery",
        }
    }
}

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // E0009 duplicate-name
    // ---------------------------------------------------------------------

    /// Report a relation name (table alias, or table name without one) that the
    /// FROM clause of a SELECT gives twice, and return those names
    pub(super) fn check_duplicate_relation_names(
        &mut self,
        from: &[TableWithJoins],
    ) -> Vec<String> {
        let mut duplicates = Vec::new();
        if !matches!(
            self.dialect,
            SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL
        ) {
            return duplicates;
        }
        let mut names: Vec<FromName> = Vec::new();
        for table in from {
            self.collect_from_names(&table.relation, &mut names);
            for join in &table.joins {
                self.collect_from_names(&join.relation, &mut names);
            }
        }
        for (i, name) in names.iter().enumerate() {
            let earlier = names[..i].iter().any(|other| {
                self.names_equal(&other.name, &name.name)
                    // `FROM auth.users, public.users`: two tables named `users`
                    && (other.table.is_none()
                        || name.table.is_none()
                        || other.table == name.table)
            });
            if earlier {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::DuplicateName,
                        format!(
                            "Table name '{}' is specified more than once in FROM",
                            name.name.value
                        ),
                    )
                    .with_span(Span::from_sqlparser(&name.name.span))
                    .with_help("Give each occurrence a different alias"),
                );
                duplicates.push(name.name.value.clone());
            }
        }
        duplicates
    }

    /// Make the columns of relations named twice in the FROM clause being resolved
    /// unknown: the later one replaced the earlier one in scope, so references to
    /// the name shouldn't cascade into column errors
    pub(super) fn forget_duplicate_relations(&mut self) {
        if self.duplicate_names.is_empty() {
            return;
        }
        let names = self.duplicate_names.clone();
        for (key, relation) in &mut self.scope.current().relations {
            if names.iter().any(|n| n.eq_ignore_ascii_case(key)) {
                relation.columns = None;
            }
        }
    }

    /// Names a FROM item introduces: its alias, or the table's name without one
    fn collect_from_names(&self, factor: &TableFactor, names: &mut Vec<FromName>) {
        match factor {
            TableFactor::Table {
                name,
                alias,
                args: None,
                ..
            } => match (alias, name.0.last()) {
                (Some(alias), _) => names.push(FromName {
                    name: alias.name.clone(),
                    table: None,
                }),
                (None, Some(last)) => names.push(FromName {
                    name: last.clone(),
                    table: Some(self.catalog.qualified_name(name).to_string()),
                }),
                (None, None) => {}
            },
            TableFactor::Derived {
                alias: Some(alias), ..
            }
            | TableFactor::NestedJoin {
                alias: Some(alias), ..
            } => names.push(FromName {
                name: alias.name.clone(),
                table: None,
            }),
            TableFactor::NestedJoin {
                table_with_joins,
                alias: None,
            } => {
                self.collect_from_names(&table_with_joins.relation, names);
                for join in &table_with_joins.joins {
                    self.collect_from_names(&join.relation, names);
                }
            }
            _ => {}
        }
    }

    /// Whether two relation names are the same: MySQL compares aliases case
    /// sensitively (on most platforms), PostgreSQL folds unquoted names
    fn names_equal(&self, a: &Ident, b: &Ident) -> bool {
        if self.dialect == SqlDialect::MySQL {
            a.value == b.value
        } else {
            self.catalog.ident_name(a) == self.catalog.ident_name(b)
        }
    }

    /// Report a CTE name that one WITH clause defines twice
    pub(super) fn check_duplicate_cte_names(&mut self, query: &Query) {
        if !matches!(
            self.dialect,
            SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL | SqlDialect::SQLite
        ) {
            return;
        }
        let Some(with) = &query.with else {
            return;
        };
        for (i, cte) in with.cte_tables.iter().enumerate() {
            let name = &cte.alias.name;
            let earlier = with.cte_tables[..i]
                .iter()
                .any(|other| self.names_equal(&other.alias.name, name));
            if earlier {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::DuplicateName,
                        format!(
                            "WITH query name '{}' is specified more than once",
                            name.value
                        ),
                    )
                    .with_span(Span::from_sqlparser(&name.span))
                    .with_help("Rename one of the CTEs"),
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // E0010 duplicate-target-column
    // ---------------------------------------------------------------------

    /// Report a column an INSERT column list names twice
    pub(super) fn check_duplicate_insert_columns(&mut self, columns: &[Ident]) {
        if !matches!(
            self.dialect,
            SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL
        ) {
            return;
        }
        for (i, column) in columns.iter().enumerate() {
            if columns[..i]
                .iter()
                .any(|other| other.value.eq_ignore_ascii_case(&column.value))
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::DuplicateTargetColumn,
                        format!(
                            "Column '{}' is specified more than once in the INSERT column list",
                            column.value
                        ),
                    )
                    .with_span(Span::from_sqlparser(&column.span))
                    .with_help("Remove the repeated column and its value"),
                );
            }
        }
    }

    /// Report a column that UPDATE SET (or ON CONFLICT DO UPDATE SET) assigns twice
    pub(super) fn check_duplicate_assignments(&mut self, assignments: &[Assignment]) {
        // MySQL and SQLite apply the assignments in order
        if !matches!(self.dialect, SqlDialect::PostgreSQL | SqlDialect::Redshift) {
            return;
        }
        // Only plain column targets: `SET c.f1 = 1, c.f2 = 2` assigns two fields
        let targets: Vec<&Ident> = assignments
            .iter()
            .filter_map(|a| match &a.target {
                AssignmentTarget::ColumnName(name) if name.0.len() == 1 => name.0.first(),
                _ => None,
            })
            .collect();
        for (i, column) in targets.iter().enumerate() {
            if targets[..i]
                .iter()
                .any(|other| self.catalog.ident_name(other) == self.catalog.ident_name(column))
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::DuplicateTargetColumn,
                        format!("Column '{}' is assigned more than once", column.value),
                    )
                    .with_span(Span::from_sqlparser(&column.span))
                    .with_help("Remove one of the assignments"),
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // E0011 position-out-of-range
    // ---------------------------------------------------------------------

    /// Report `ORDER BY n` past the end of a query's `count` output columns
    pub(super) fn check_order_by_positions(
        &mut self,
        query: &Query,
        count: usize,
        at: Option<Span>,
    ) {
        if !self.dialect.checks_ordinal_positions() {
            return;
        }
        let Some(order_by) = &query.order_by else {
            return;
        };
        for item in &order_by.exprs {
            self.check_position(&item.expr, count, "ORDER BY", at);
        }
    }

    /// Report `GROUP BY n` past the end of a SELECT's `count` output columns
    pub(super) fn check_group_by_positions(&mut self, select: &Select, count: usize) {
        if !self.dialect.checks_ordinal_positions() {
            return;
        }
        if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
            let at = Some(Span::from_sqlparser(&select.select_token.0.span));
            for expr in exprs {
                self.check_position(expr, count, "GROUP BY", at);
            }
        }
    }

    fn check_position(&mut self, expr: &Expr, count: usize, clause: &str, at: Option<Span>) {
        let Expr::Value(Value::Number(n, _)) = expr else {
            return;
        };
        let Ok(position) = n.parse::<usize>() else {
            return;
        };
        if (1..=count).contains(&position) {
            return;
        }
        let span = Some(Span::from_sqlparser(&expr.span()))
            .filter(|s| s.line > 0)
            .or(at);
        let mut diag = Diagnostic::error(
            DiagnosticKind::PositionOutOfRange,
            format!("{clause} position {position} is not in the select list"),
        );
        diag.span = span;
        self.diagnostics.push(diag.with_help(format!(
            "The select list has {count} column{}; positions count from 1",
            if count == 1 { "" } else { "s" }
        )));
    }

    // ---------------------------------------------------------------------
    // E0012 misplaced-aggregate
    // ---------------------------------------------------------------------

    /// Report aggregate or window functions in a clause that doesn't allow them
    pub(super) fn check_aggregates_in(&mut self, expr: &Expr, clause: Clause) {
        let mut finder = FunctionFinder::new(self.dialect);
        let _ = expr.visit(&mut finder);
        for found in finder.found {
            let (message, help) = match found.kind {
                Found::Window => (
                    format!(
                        "Window function '{}' is not allowed in {}",
                        found.name.value,
                        clause.name()
                    ),
                    "Compute the window function in a subquery or CTE and filter its result"
                        .to_string(),
                ),
                Found::Aggregate if clause.allows_aggregates() => continue,
                Found::Aggregate => (
                    format!(
                        "Aggregate function '{}' is not allowed in {}",
                        found.name.value,
                        clause.name()
                    ),
                    clause.aggregate_help().to_string(),
                ),
                Found::Nested => (
                    format!(
                        "Aggregate function '{}' is nested inside another aggregate",
                        found.name.value
                    ),
                    "Aggregate in a subquery first, then aggregate its result".to_string(),
                ),
            };
            self.diagnostics.push(
                Diagnostic::error(DiagnosticKind::MisplacedAggregate, message)
                    .with_span(Span::from_sqlparser(&found.name.span))
                    .with_help(help),
            );
        }
    }

    /// Report aggregates nested in aggregates (in clauses that allow aggregates)
    pub(super) fn check_nested_aggregates(&mut self, expr: &Expr) {
        let mut finder = FunctionFinder::new(self.dialect);
        let _ = expr.visit(&mut finder);
        for found in finder.found {
            if let Found::Nested = found.kind {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::MisplacedAggregate,
                        format!(
                            "Aggregate function '{}' is nested inside another aggregate",
                            found.name.value
                        ),
                    )
                    .with_span(Span::from_sqlparser(&found.name.span))
                    .with_help("Aggregate in a subquery first, then aggregate its result"),
                );
            }
        }
    }
}

/// A name a FROM item introduces, with the table it names when it isn't aliased
struct FromName {
    name: Ident,
    table: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum Found {
    /// An aggregate call outside any aggregate
    Aggregate,
    /// An aggregate call inside another aggregate's arguments
    Nested,
    /// A window function call (outside any aggregate)
    Window,
}

struct FoundFunction {
    name: Ident,
    kind: Found,
}

/// Finds aggregate and window function calls of an expression, not looking into
/// subqueries (their aggregates belong to them)
struct FunctionFinder {
    dialect: SqlDialect,
    /// Depth of subqueries being visited
    query_depth: usize,
    /// Depth of aggregate calls being visited (outside subqueries)
    aggregate_depth: usize,
    found: Vec<FoundFunction>,
}

impl FunctionFinder {
    fn new(dialect: SqlDialect) -> Self {
        Self {
            dialect,
            query_depth: 0,
            aggregate_depth: 0,
            found: Vec::new(),
        }
    }
}

impl Visitor for FunctionFinder {
    type Break = ();

    fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<()> {
        self.query_depth += 1;
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<()> {
        self.query_depth -= 1;
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        let Expr::Function(func) = expr else {
            return ControlFlow::Continue(());
        };
        if self.query_depth > 0 {
            return ControlFlow::Continue(());
        }
        let Some(name) = func.name.0.last() else {
            return ControlFlow::Continue(());
        };
        if func.over.is_some() {
            // A window function over aggregates (`sum(count(*)) OVER ()`) is fine
            if self.aggregate_depth == 0 {
                self.found.push(FoundFunction {
                    name: name.clone(),
                    kind: Found::Window,
                });
            }
        } else if is_aggregate(func, self.dialect) {
            let kind = if self.aggregate_depth > 0 {
                Found::Nested
            } else {
                Found::Aggregate
            };
            self.found.push(FoundFunction {
                name: name.clone(),
                kind,
            });
            self.aggregate_depth += 1;
        }
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        if let Expr::Function(func) = expr {
            if self.query_depth == 0 && func.over.is_none() && is_aggregate(func, self.dialect) {
                self.aggregate_depth -= 1;
            }
        }
        ControlFlow::Continue(())
    }
}

/// Whether a call (without OVER) is a built-in aggregate function. Only unqualified
/// names: `myschema.sum(x)` may be a user-defined function.
pub(super) fn is_aggregate(func: &Function, dialect: SqlDialect) -> bool {
    let [name] = func.name.0.as_slice() else {
        return false;
    };
    if name.quote_style.is_some() {
        return false;
    }
    let args = match &func.args {
        FunctionArguments::List(list) => list.args.len(),
        FunctionArguments::None => return false,
        FunctionArguments::Subquery(_) => return false,
    };
    // FILTER (WHERE ...) and WITHIN GROUP (...) only apply to aggregates
    if func.filter.is_some() || !func.within_group.is_empty() {
        return true;
    }
    match name.value.to_ascii_lowercase().as_str() {
        // SQLite's (and MySQL's) min/max with several arguments are scalar
        "min" | "max" => args == 1,
        "count" | "sum" | "avg" | "array_agg" | "string_agg" | "bool_and" | "bool_or" | "every"
        | "bit_and" | "bit_or" | "bit_xor" | "json_agg" | "jsonb_agg" | "json_object_agg"
        | "jsonb_object_agg" | "stddev" | "stddev_pop" | "stddev_samp" | "variance" | "var_pop"
        | "var_samp" | "corr" | "covar_pop" | "covar_samp" | "regr_avgx" | "regr_avgy"
        | "regr_count" | "regr_intercept" | "regr_r2" | "regr_slope" | "regr_sxx" | "regr_sxy"
        | "regr_syy" | "xmlagg" => true,
        "group_concat" => matches!(dialect, SqlDialect::MySQL | SqlDialect::SQLite),
        "json_arrayagg" | "json_objectagg" => dialect == SqlDialect::MySQL,
        "json_group_array" | "json_group_object" | "total" => dialect == SqlDialect::SQLite,
        _ => false,
    }
}
