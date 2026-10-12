//! Structural checks of the statement resolver: rules about where things may appear
//! and how often, which need the query's shape rather than its types
//!
//! Checks:
//! - a table alias or CTE name given twice in one query block (E0009)
//! - a column given twice in an INSERT column list or UPDATE SET (E0010)
//! - an ORDER BY / GROUP BY position past the end of the select list (E0011)
//! - aggregate and window functions where SQL doesn't allow them (E0012)
//! - a value given for a generated column (E0013)
//! - an ON CONFLICT target that matches no unique key (E0014)
//! - column alias lists longer than the relation, subqueries and row
//!   constructors with the wrong number of columns (E0005)
//! - ORDER BY of SELECT DISTINCT on something not selected (E0015)
//!
//! Each check is limited to the dialects known to reject the query.

use std::ops::ControlFlow;

use sqlsift_ast::{
    Assignment, AssignmentTarget, Expr, Function, FunctionArguments, GroupByExpr, Ident, Insert,
    Parens, Query, Select, SelectItem, Spanned, TableFactor, TableWithJoins, Value, ValueWithSpan,
    Visit, Visitor,
};

use crate::ast_ext::{column_ident, insert_table, name_last, order_by_exprs, ObjectNamePartExt};
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{ColumnDef, IdentityKind, QualifiedName, TableDef};

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
            } => match (alias, name_last(name)) {
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
                AssignmentTarget::ColumnName(name) if name.0.len() == 1 => name_last(name),
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
        for item in order_by_exprs(order_by) {
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
        let Expr::Value(ValueWithSpan {
            value: Value::Number(n, _),
            ..
        }) = expr
        else {
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
        let _ = visit_flat(expr, &mut finder);
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
        let _ = visit_flat(expr, &mut finder);
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

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // E0013 generated-column-write
    // ---------------------------------------------------------------------

    /// Columns an INSERT without a column list gives values for, in order: all
    /// columns, but SQLite leaves out generated columns
    pub(super) fn positional_columns<'t>(&self, table: &'t TableDef) -> Vec<&'t str> {
        table
            .columns
            .values()
            .filter(|c| !(self.dialect == SqlDialect::SQLite && c.generated))
            .map(|c| c.name.as_str())
            .collect()
    }

    /// Why a column can't be given a value other than DEFAULT, if it can't
    fn generated_kind(&self, column: &ColumnDef) -> Option<&'static str> {
        if column.generated {
            Some("a generated column")
        } else if matches!(column.identity, Some(IdentityKind::Always))
            && matches!(self.dialect, SqlDialect::PostgreSQL)
        {
            Some("an identity column GENERATED ALWAYS")
        } else {
            None
        }
    }

    /// Report a value other than DEFAULT given for a generated column by an INSERT
    /// (`rows` of VALUES, or `None` for INSERT ... SELECT)
    pub(super) fn check_generated_insert(
        &mut self,
        insert: &Insert,
        table: &TableDef,
        rows: Option<&[Parens<Vec<Expr>>]>,
    ) {
        let targets: Vec<(&str, Option<Span>)> = if insert.columns.is_empty() {
            self.positional_columns(table)
                .into_iter()
                .map(|c| (c, None))
                .collect()
        } else {
            insert
                .columns
                .iter()
                .map(column_ident)
                .map(|c| (c.value.as_str(), Some(Span::from_sqlparser(&c.span))))
                .collect()
        };
        let table_span = insert_table(insert)
            .and_then(name_last)
            .map(|id| Span::from_sqlparser(&id.span));
        for (i, (name, span)) in targets.iter().enumerate() {
            let Some(column) = table.get_column(name) else {
                continue;
            };
            let Some(kind) = self.generated_kind(column) else {
                continue;
            };
            // INSERT ... SELECT can't give DEFAULT; VALUES can
            let given = rows.map_or(!insert.columns.is_empty(), |rows| {
                rows.iter()
                    .any(|row| row.get(i).is_some_and(|value| !is_default(value)))
            });
            if !given {
                continue;
            }
            let mut diag = Diagnostic::error(
                DiagnosticKind::GeneratedColumnWrite,
                format!(
                    "Column '{}' is {kind}: INSERT can't give it a value",
                    column.name
                ),
            )
            .with_help("Leave the column out of the INSERT, or give it DEFAULT");
            diag.span = span.or(table_span);
            self.diagnostics.push(diag);
        }
    }

    /// Report UPDATE SET of a generated column to a value other than DEFAULT
    pub(super) fn check_generated_update(&mut self, table: &TableDef, assignments: &[Assignment]) {
        for assignment in assignments {
            let AssignmentTarget::ColumnName(name) = &assignment.target else {
                continue;
            };
            let [ident] = name.0.as_slice() else {
                continue;
            };
            let Some(column) = table.get_column(&ident.ident().value) else {
                continue;
            };
            let Some(kind) = self.generated_kind(column) else {
                continue;
            };
            if is_default(&assignment.value) {
                continue;
            }
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticKind::GeneratedColumnWrite,
                    format!(
                        "Column '{}' is {kind}: UPDATE can only set it to DEFAULT",
                        column.name
                    ),
                )
                .with_span(Span::from_sqlparser(&ident.ident().span))
                .with_help("Update the columns it is computed from instead"),
            );
        }
    }

    // ---------------------------------------------------------------------
    // E0014 unmatched-conflict-target
    // ---------------------------------------------------------------------

    /// Report `ON CONFLICT (columns)` when no primary key or unique constraint of the
    /// table has exactly those columns
    pub(super) fn check_conflict_target(
        &mut self,
        table: &TableDef,
        table_name: &QualifiedName,
        columns: &[Ident],
    ) {
        if !matches!(self.dialect, SqlDialect::PostgreSQL | SqlDialect::SQLite)
            || columns.is_empty()
            || table.unknown_unique_keys
            || self.catalog.unknown_unique_keys
            || !columns.iter().all(|c| table.column_exists(&c.value))
        {
            return;
        }
        let same_columns = |key: &[String]| {
            key.len() == columns.len()
                && key
                    .iter()
                    .all(|k| columns.iter().any(|c| c.value.eq_ignore_ascii_case(k)))
                && columns
                    .iter()
                    .all(|c| key.iter().any(|k| k.eq_ignore_ascii_case(&c.value)))
        };
        let primary_key: Vec<String> = match &table.primary_key {
            Some(pk) => pk.columns.clone(),
            None => table
                .columns
                .values()
                .filter(|c| c.is_primary_key)
                .map(|c| c.name.clone())
                .collect(),
        };
        let matches = (!primary_key.is_empty() && same_columns(&primary_key))
            || table
                .unique_constraints
                .iter()
                .any(|u| same_columns(&u.columns));
        if matches {
            return;
        }
        let list = columns
            .iter()
            .map(|c| c.value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let keys: Vec<String> = std::iter::once(primary_key)
            .filter(|k| !k.is_empty())
            .chain(table.unique_constraints.iter().map(|u| u.columns.clone()))
            .map(|k| format!("({})", k.join(", ")))
            .collect();
        let help = if keys.is_empty() {
            format!("Table '{table_name}' has no primary key or unique constraint")
        } else {
            format!(
                "The unique keys of '{table_name}' are {}; add a unique constraint or index on ({list})",
                keys.join(", ")
            )
        };
        self.diagnostics.push(
            Diagnostic::error(
                DiagnosticKind::UnmatchedConflictTarget,
                format!("No unique constraint or primary key of '{table_name}' matches ON CONFLICT ({list})"),
            )
            .with_span(Span::from_sqlparser(&columns[0].span))
            .with_help(help),
        );
    }
}

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // E0005 column-count-mismatch (column aliases, subqueries, rows)
    // ---------------------------------------------------------------------

    /// Report a column alias list (`t AS x(a, b)`, `WITH x(a, b) AS ...`) with more
    /// names than the relation has columns, or for MySQL and SQLite CTEs and
    /// derived tables, a different number. `what` is "Table" or "WITH query".
    pub(super) fn check_alias_count(
        &mut self,
        what: &str,
        name: &Ident,
        columns: Option<&[super::scope::Column]>,
        aliases: usize,
        exact: bool,
    ) {
        if !matches!(
            self.dialect,
            SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL | SqlDialect::SQLite
        ) {
            return;
        }
        let Some(columns) = columns else {
            return;
        };
        let exact = exact && matches!(self.dialect, SqlDialect::MySQL | SqlDialect::SQLite);
        if aliases == 0 || aliases == columns.len() || (aliases < columns.len() && !exact) {
            return;
        }
        self.diagnostics.push(
            Diagnostic::error(
                DiagnosticKind::ColumnCountMismatch,
                format!(
                    "{what} '{}' has {} column{} but {aliases} column name{} were given",
                    name.value,
                    columns.len(),
                    if columns.len() == 1 { "" } else { "s" },
                    if aliases == 1 { "" } else { "s" },
                ),
            )
            .with_span(Span::from_sqlparser(&name.span))
            .with_help(if exact {
                "Give one name per column"
            } else {
                "Give at most one name per column"
            }),
        );
    }

    /// Whether subquery and row shapes are checked (E0005)
    fn checks_subquery_shapes(&self) -> bool {
        matches!(
            self.dialect,
            SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL | SqlDialect::SQLite
        )
    }

    /// Note a subquery compared with or assigned to a row (`(a, b) = (SELECT ...)`,
    /// `SET (a, b) = (SELECT ...)`), which may return several columns
    pub(super) fn allow_row_subquery(&mut self, expr: &Expr) {
        if let Expr::Subquery(query) = expr {
            self.row_subqueries
                .push(query.as_ref() as *const Query as usize);
        }
    }

    /// Note the subqueries of `left op right` that are compared with a row
    pub(super) fn note_row_comparison(&mut self, left: &Expr, right: &Expr) {
        if matches!(left, Expr::Tuple(_)) {
            self.allow_row_subquery(right);
        }
        if matches!(right, Expr::Tuple(_)) {
            self.allow_row_subquery(left);
        }
    }

    fn subquery_column_count(&self, query: &Query) -> Option<usize> {
        let columns = self
            .subquery_columns
            .get(&(query as *const Query as usize))?;
        columns.as_ref().map(Vec::len)
    }

    fn report_shape(&mut self, message: String, help: &str, at: &Expr) {
        let mut diag =
            Diagnostic::error(DiagnosticKind::ColumnCountMismatch, message).with_help(help);
        diag.span = Some(Span::from_sqlparser(&at.span())).filter(|s| s.line > 0);
        self.diagnostics.push(diag);
    }

    /// Report a row compared with a row or subquery of another length (after both
    /// operands are resolved)
    pub(super) fn check_row_comparison(&mut self, left: &Expr, right: &Expr) {
        if !self.checks_subquery_shapes() {
            return;
        }
        match (left, right) {
            (Expr::Tuple(a), Expr::Tuple(b)) if a.len() != b.len() => {
                self.report_shape(
                    format!(
                        "Row comparison of {} values with {} values",
                        a.len(),
                        b.len()
                    ),
                    "Compare rows with the same number of values",
                    left,
                );
            }
            (Expr::Tuple(row), Expr::Subquery(query))
            | (Expr::Subquery(query), Expr::Tuple(row)) => {
                if let Some(n) = self.subquery_column_count(query) {
                    if n != row.len() {
                        self.report_shape(
                            format!(
                                "Row of {} values compared with a subquery of {n} columns",
                                row.len()
                            ),
                            "Select as many columns as the row has values",
                            left,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Report `x IN (SELECT ...)` whose subquery returns a different number of
    /// columns than `x` has values
    pub(super) fn check_in_subquery_shape(&mut self, expr: &Expr, columns: Option<usize>) {
        if !self.checks_subquery_shapes() {
            return;
        }
        let expected = match expr {
            Expr::Tuple(row) => row.len(),
            _ => 1,
        };
        if let Some(n) = columns.filter(|&n| n != expected) {
            self.report_shape(
                format!(
                    "IN subquery returns {n} columns but {expected} {} expected",
                    if expected == 1 { "is" } else { "are" }
                ),
                "Select only the column to compare with",
                expr,
            );
        }
    }

    /// Report a subquery used as a value that returns several columns
    pub(super) fn check_scalar_subquery(&mut self, expr: &Expr, query: &Query) {
        if !self.checks_subquery_shapes()
            || self
                .row_subqueries
                .contains(&(query as *const Query as usize))
        {
            return;
        }
        if let Some(n) = self.subquery_column_count(query).filter(|&n| n > 1) {
            self.report_shape(
                format!("Subquery used as a value returns {n} columns"),
                "A scalar subquery must select exactly one column",
                expr,
            );
        }
    }
}

/// Visit an expression like [`Visit::visit`], walking chains of binary operators
/// (`a OR b OR ...`, which nest as deep as they are long) in a loop, so that a long
/// chain doesn't overflow the stack
pub(super) fn visit_flat<V: Visitor>(expr: &Expr, visitor: &mut V) -> ControlFlow<V::Break> {
    match expr {
        Expr::BinaryOp { .. } => {
            let mut spine = Vec::new();
            let mut leftmost = expr;
            while let Expr::BinaryOp { left, .. } = leftmost {
                spine.push(leftmost);
                leftmost = left;
            }
            for node in &spine {
                visitor.pre_visit_expr(node)?;
            }
            visit_flat(leftmost, visitor)?;
            for node in spine.iter().rev() {
                if let Expr::BinaryOp { right, .. } = node {
                    visit_flat(right, visitor)?;
                }
                visitor.post_visit_expr(node)?;
            }
            ControlFlow::Continue(())
        }
        Expr::Nested(inner) => {
            visitor.pre_visit_expr(expr)?;
            visit_flat(inner, visitor)?;
            visitor.post_visit_expr(expr)
        }
        _ => expr.visit(visitor),
    }
}

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // E0015 distinct-order-by
    // ---------------------------------------------------------------------

    /// Report ORDER BY of a SELECT DISTINCT on a column the select list doesn't
    /// have, and DISTINCT ON expressions that don't start the ORDER BY
    pub(super) fn check_distinct_order_by(&mut self, select: &Select, query: &Query) {
        let Some(order_by) = &query.order_by else {
            return;
        };
        match &select.distinct {
            Some(sqlsift_ast::Distinct::Distinct)
                if matches!(
                    self.dialect,
                    SqlDialect::PostgreSQL | SqlDialect::Redshift | SqlDialect::MySQL
                ) =>
            {
                // `*` and `t.*` select every column of something
                let mut selected: Vec<String> = Vec::new();
                for item in &select.projection {
                    match item {
                        SelectItem::Wildcard(_)
                        | SelectItem::QualifiedWildcard(..)
                        | SelectItem::ExprWithAliases { .. } => return,
                        SelectItem::ExprWithAlias { expr, alias } => {
                            selected.push(alias.value.to_lowercase());
                            if let Some(name) = column_ref_name(expr) {
                                selected.push(name);
                            }
                        }
                        SelectItem::UnnamedExpr(expr) => {
                            if let Some(name) = column_ref_name(expr) {
                                selected.push(name);
                            }
                        }
                    }
                }
                for item in order_by_exprs(order_by) {
                    let Some(name) = column_ref_name(&item.expr) else {
                        continue;
                    };
                    if selected.contains(&name) {
                        continue;
                    }
                    let mut diag = Diagnostic::error(
                        DiagnosticKind::DistinctOrderBy,
                        format!("ORDER BY '{}' is not in the select list of SELECT DISTINCT", item.expr),
                    )
                    .with_help("With DISTINCT, ORDER BY can only use selected columns: select it too, or drop DISTINCT");
                    diag.span =
                        Some(Span::from_sqlparser(&item.expr.span())).filter(|s| s.line > 0);
                    self.diagnostics.push(diag);
                }
            }
            Some(sqlsift_ast::Distinct::On(on)) if self.dialect == SqlDialect::PostgreSQL => {
                let Some(on_names) = on.iter().map(column_ref_name).collect::<Option<Vec<_>>>()
                else {
                    return;
                };
                let mut covered: Vec<&String> = Vec::new();
                for item in order_by_exprs(order_by) {
                    if on_names.iter().all(|n| covered.contains(&n)) {
                        break;
                    }
                    let Some(name) = column_ref_name(&item.expr) else {
                        return;
                    };
                    if let Some(n) = on_names.iter().find(|n| **n == name) {
                        covered.push(n);
                        continue;
                    }
                    let mut diag = Diagnostic::error(
                        DiagnosticKind::DistinctOrderBy,
                        format!(
                            "ORDER BY '{}' comes before all DISTINCT ON expressions",
                            item.expr
                        ),
                    )
                    .with_help(
                        "Start ORDER BY with the DISTINCT ON expressions, then order by other columns",
                    );
                    diag.span =
                        Some(Span::from_sqlparser(&item.expr.span())).filter(|s| s.line > 0);
                    self.diagnostics.push(diag);
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Lowercase name of the column a plain column reference (`c`, `t.c`) names
fn column_ref_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.to_lowercase()),
        Expr::CompoundIdentifier(idents) => idents.last().map(|i| i.value.to_lowercase()),
        Expr::Nested(inner) => column_ref_name(inner),
        _ => None,
    }
}

/// Whether an expression is the DEFAULT keyword (which the parser represents as an
/// identifier)
fn is_default(expr: &Expr) -> bool {
    matches!(expr, Expr::Identifier(ident)
        if ident.quote_style.is_none() && ident.value.eq_ignore_ascii_case("DEFAULT"))
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
        let Some(name) = name_last(&func.name) else {
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
    if name.ident().quote_style.is_some() {
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
    match name.ident().value.to_ascii_lowercase().as_str() {
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
