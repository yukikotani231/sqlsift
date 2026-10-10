//! Statement resolver - resolves table and column references and checks types
//!
//! A single walk over each statement: FROM clauses build the [`Scope`], names are
//! resolved against it, and every query block reports its output columns with their
//! types, so CTEs, subqueries and views carry column types into the queries that use
//! them. The type checks themselves live in `type_check.rs`.

use sqlparser::ast::{
    Assignment, AssignmentTarget, ConflictTarget, Delete, Distinct, Expr, GroupByExpr, Ident,
    Insert, JoinConstraint, JoinOperator, NamedWindowDefinition, NamedWindowExpr, OnConflictAction,
    OnInsert, Query, Select, SelectItem, SetExpr, Statement, Subscript, TableFactor,
    TableWithJoins, Values,
};
use std::collections::HashMap;

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{Catalog, ColumnDef, FormerColumn, QualifiedName, TableDef};
use crate::suggest::{find_most_similar, find_similar_name};

use super::scope::{
    lookup_ignore_case, Column, ColumnLookup, ColumnMatch, Cte, ExpressionType, Relation,
    RelationKind, Scope, UNNAMED_COLUMN,
};

/// Resolves names and checks types in SQL statements
pub(super) struct Resolver<'a> {
    pub(super) catalog: &'a Catalog,
    pub(super) dialect: SqlDialect,
    /// Relations and CTEs visible at the current point of the walk
    pub(super) scope: Scope,
    /// Collected diagnostics
    pub(super) diagnostics: Vec<Diagnostic>,
    /// Output columns of subqueries in expressions, keyed by the query's address, so
    /// type inference can use them without walking the subquery again
    pub(super) subquery_columns: HashMap<usize, Option<Vec<Column>>>,
    /// Number of unaliased derived tables seen (to give each a distinct scope key)
    unaliased_subqueries: usize,
}

impl<'a> Resolver<'a> {
    pub(super) fn new(catalog: &'a Catalog, dialect: SqlDialect) -> Self {
        Self {
            catalog,
            dialect,
            scope: Scope::default(),
            diagnostics: Vec::new(),
            subquery_columns: HashMap::new(),
            unaliased_subqueries: 0,
        }
    }

    /// Consume the resolver and return collected diagnostics
    pub(super) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Resolve a statement. Returns its output columns (the result of a query, or
    /// the RETURNING list of INSERT/UPDATE/DELETE) if they can be determined.
    pub(super) fn statement(&mut self, stmt: &Statement) -> Option<Vec<Column>> {
        match stmt {
            Statement::Query(query) => self.query(query, false),
            Statement::Insert(insert) => {
                self.scope.push(false);
                let columns = self.insert(insert);
                self.scope.pop();
                columns
            }
            Statement::Update {
                table,
                assignments,
                from,
                selection,
                returning,
                ..
            } => {
                self.scope.push(false);
                self.update(table, assignments, from.as_ref(), selection.as_ref());
                let columns = self.returning(returning.as_deref());
                self.scope.pop();
                columns
            }
            Statement::Delete(delete) => {
                self.scope.push(false);
                let columns = self.delete(delete);
                self.scope.pop();
                columns
            }
            // The queries of CREATE TABLE ... AS and CREATE VIEW (the relations they
            // define are applied to the file's catalog by the analyzer)
            Statement::CreateTable(create) => {
                if let Some(query) = &create.query {
                    self.query(query, false);
                }
                None
            }
            Statement::CreateView { query, .. } => {
                self.query(query, false);
                None
            }
            _ => None,
        }
    }

    // ---------------------------------------------------------------------
    // DML
    // ---------------------------------------------------------------------

    /// Resolve an INSERT statement in the current (statement) frame
    fn insert(&mut self, insert: &Insert) -> Option<Vec<Column>> {
        let table_name = self.catalog.qualified_name(&insert.table_name);
        let table_span = insert
            .table_name
            .0
            .last()
            .map(|id| Span::from_sqlparser(&id.span));

        let Some(table_def) = self.catalog.get_table(&table_name) else {
            if let Some(view) = self.catalog.get_view(&table_name) {
                // Simple views are insertable: check the column list against the view
                if !view.columns.is_empty() {
                    for col_ident in &insert.columns {
                        if !view
                            .columns
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(&col_ident.value))
                        {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticKind::ColumnNotFound,
                                    format!(
                                        "Column '{}' not found in view '{}'",
                                        col_ident.value, table_name
                                    ),
                                )
                                .with_span(Span::from_sqlparser(&col_ident.span)),
                            );
                        }
                    }
                }
                if let Some(source) = &insert.source {
                    self.insert_source(source);
                }
            } else {
                let diag = self.table_not_found(&table_name, table_span);
                self.diagnostics.push(diag);
            }
            return None;
        };

        // Check if specified columns exist
        for col_ident in &insert.columns {
            if !table_def.column_exists(&col_ident.value) {
                let mut diag = Diagnostic::error(
                    DiagnosticKind::ColumnNotFound,
                    format!(
                        "Column '{}' not found in table '{}'",
                        col_ident.value, table_name
                    ),
                )
                .with_span(Span::from_sqlparser(&col_ident.span));
                if let Some(help) = missing_table_column_help(table_def, &col_ident.value) {
                    diag = diag.with_help(help);
                }
                self.diagnostics.push(diag);
            }
        }

        let expected_count = if insert.columns.is_empty() {
            table_def.columns.len()
        } else {
            insert.columns.len()
        };
        let values = insert
            .source
            .as_deref()
            .and_then(|q| match q.body.as_ref() {
                SetExpr::Values(Values { rows, .. }) => Some(rows),
                _ => None,
            });

        // INSERT ... SELECT: the source query has its own scope
        let selected = match (&insert.source, values) {
            (Some(source), None) => self.insert_source(source),
            _ => None,
        };

        // NOT NULL columns without a default must be given a value. Only checked
        // with an explicit column list (or DEFAULT VALUES); without one, every
        // column is positional and E0005 covers missing values.
        // Skipped when the column list or VALUES arity is already wrong: the missing
        // column is then usually the one that was misspelled or miscounted.
        // (VALUES literals carry no source location, so point at the table name.)
        let default_values = insert.source.is_none() && insert.columns.is_empty();
        let columns_valid = insert
            .columns
            .iter()
            .all(|c| table_def.column_exists(&c.value));
        let arity_valid = match (values, &selected) {
            (Some(rows), _) => rows.iter().all(|row| row.len() == insert.columns.len()),
            (None, Some(selected)) => selected.len() == insert.columns.len(),
            (None, None) => true,
        };
        if (!insert.columns.is_empty() || default_values) && columns_valid && arity_valid {
            self.check_required_columns(insert, table_def, &table_name, table_span);
        }

        if let Some(rows) = values {
            for row in rows {
                if row.len() != expected_count {
                    let mut diag = Diagnostic::error(
                        DiagnosticKind::ColumnCountMismatch,
                        format!(
                            "INSERT has {} value(s) but {} column(s) were specified",
                            row.len(),
                            expected_count
                        ),
                    )
                    .with_help(if insert.columns.is_empty() {
                        format!(
                            "Table '{table_name}' has {expected_count} columns. Specify columns explicitly or provide {expected_count} values"
                        )
                    } else {
                        format!("Provide {expected_count} value(s) to match the column list")
                    });
                    diag.span = table_span;
                    self.diagnostics.push(diag);
                }
                for expr in row {
                    self.expr(expr);
                }
            }
            self.check_insert_values(insert, table_def, rows);
        } else if let Some(selected) = &selected {
            if selected.len() != expected_count {
                let mut diag = Diagnostic::error(
                    DiagnosticKind::ColumnCountMismatch,
                    format!(
                        "INSERT ... SELECT returns {} column(s) but {} column(s) were specified",
                        selected.len(),
                        expected_count
                    ),
                )
                .with_help(format!(
                    "Select {expected_count} column(s) to match the column list"
                ));
                diag.span = table_span;
                self.diagnostics.push(diag);
            }
        }

        // ON CONFLICT / ON DUPLICATE KEY UPDATE and RETURNING see the target table
        let target = Relation::table(self.catalog, &table_name)?;
        let key = insert
            .table_alias
            .as_ref()
            .map_or_else(|| table_name.name.clone(), |a| a.value.clone());
        self.scope.current().relations.insert(key, target.clone());
        if let Some(on) = &insert.on {
            // MySQL row alias: INSERT ... VALUES (...) AS new ON DUPLICATE KEY UPDATE c = new.c
            let row_alias = insert
                .insert_alias
                .as_ref()
                .and_then(|a| a.row_alias.0.last())
                .map(|a| a.value.clone());
            if let Some(row_alias) = &row_alias {
                self.scope
                    .current()
                    .relations
                    .insert(row_alias.clone(), target.clone());
            }
            self.on_insert(on, table_def, &table_name, target);
            // EXCLUDED / the row alias are only visible in the ON clause
            let relations = &mut self.scope.current().relations;
            relations.shift_remove("excluded");
            if let Some(row_alias) = &row_alias {
                relations.shift_remove(row_alias);
            }
        }
        self.returning(insert.returning.as_deref())
    }

    /// Resolve the source query of INSERT ... SELECT, which can't see the target table
    fn insert_source(&mut self, source: &Query) -> Option<Vec<Column>> {
        self.query(source, true)
    }

    /// Report NOT NULL columns without a default that an INSERT omits (E0008)
    fn check_required_columns(
        &mut self,
        insert: &Insert,
        table_def: &TableDef,
        table_name: &QualifiedName,
        span: Option<Span>,
    ) {
        let missing: Vec<&str> = table_def
            .columns
            .values()
            .filter(|col| {
                !insert
                    .columns
                    .iter()
                    .any(|c| c.value.eq_ignore_ascii_case(&col.name))
                    && !self.column_has_implicit_value(table_def, col)
            })
            .map(|col| col.name.as_str())
            .collect();
        if missing.is_empty() {
            return;
        }
        let list = missing
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut diag = Diagnostic::error(
            DiagnosticKind::MissingRequiredColumn,
            format!(
                "INSERT into '{}' is missing required column{} {}",
                table_name,
                if missing.len() == 1 { "" } else { "s" },
                list
            ),
        )
        .with_help(format!(
            "{} NOT NULL without a default. Provide a value, or add a DEFAULT to the schema",
            if missing.len() == 1 {
                format!("{list} is")
            } else {
                format!("{list} are")
            }
        ));
        diag.span = span;
        self.diagnostics.push(diag);
    }

    /// Resolve `ON CONFLICT ...` / `ON DUPLICATE KEY UPDATE ...` of an INSERT
    fn on_insert(
        &mut self,
        on: &OnInsert,
        table_def: &TableDef,
        table_name: &QualifiedName,
        target: Relation,
    ) {
        let assignments = match on {
            OnInsert::DuplicateKeyUpdate(assignments) => assignments,
            OnInsert::OnConflict(on_conflict) => {
                if let Some(ConflictTarget::Columns(columns)) = &on_conflict.conflict_target {
                    for col in columns {
                        self.check_target_column(table_def, table_name, col);
                    }
                }
                match &on_conflict.action {
                    OnConflictAction::DoNothing => return,
                    OnConflictAction::DoUpdate(update) => {
                        // EXCLUDED is the row proposed for insertion
                        self.scope
                            .current()
                            .relations
                            .insert("excluded".to_string(), target);
                        if let Some(selection) = &update.selection {
                            self.expr(selection);
                        }
                        &update.assignments
                    }
                }
            }
            _ => return,
        };
        for assignment in assignments {
            if let AssignmentTarget::ColumnName(name) = &assignment.target {
                if let Some(col) = name.0.last() {
                    self.check_target_column(table_def, table_name, col);
                }
            }
            self.expr(&assignment.value);
        }
        self.check_assignments(table_def, assignments);
    }

    /// Report a column of the target table that doesn't exist
    fn check_target_column(
        &mut self,
        table_def: &TableDef,
        table_name: &QualifiedName,
        col: &Ident,
    ) {
        if table_def.column_exists(&col.value) {
            return;
        }
        let mut diag = Diagnostic::error(
            DiagnosticKind::ColumnNotFound,
            format!("Column '{}' not found in table '{}'", col.value, table_name),
        )
        .with_span(Span::from_sqlparser(&col.span));
        if let Some(help) = missing_table_column_help(table_def, &col.value) {
            diag = diag.with_help(help);
        }
        self.diagnostics.push(diag);
    }

    /// Resolve a RETURNING list against the statement's tables
    fn returning(&mut self, returning: Option<&[SelectItem]>) -> Option<Vec<Column>> {
        let items = returning?;
        let span = Span::with_location(0, 0, 0);
        for item in items {
            self.select_item(item, &span);
        }
        self.projection_columns(items)
    }

    /// Resolve an UPDATE statement in the current (statement) frame
    fn update(
        &mut self,
        table: &TableWithJoins,
        assignments: &[Assignment],
        from: Option<&TableWithJoins>,
        selection: Option<&Expr>,
    ) {
        self.table_with_joins(table);
        // PostgreSQL: UPDATE ... FROM ...
        if let Some(from_table) = from {
            self.table_with_joins(from_table);
        }

        let table_name = match &table.relation {
            TableFactor::Table { name, .. } => Some(self.catalog.qualified_name(name)),
            _ => None,
        };
        let table_def = table_name.as_ref().and_then(|n| self.catalog.get_table(n));

        for assignment in assignments {
            match &assignment.target {
                AssignmentTarget::ColumnName(col_name) if col_name.0.len() >= 2 => {
                    // `SET alias.col = ...` (MySQL multi-table UPDATE)
                    let n = col_name.0.len();
                    self.column(Some(&col_name.0[n - 2]), &col_name.0[n - 1]);
                }
                AssignmentTarget::ColumnName(col_name) if !table.joins.is_empty() => {
                    // Unqualified target in a multi-table UPDATE: any joined table
                    if let Some(col_ident) = col_name.0.last() {
                        self.column(None, col_ident);
                    }
                }
                AssignmentTarget::ColumnName(col_name) => {
                    if let (Some(col_ident), Some(def), Some(name)) =
                        (col_name.0.last(), table_def, &table_name)
                    {
                        self.check_target_column(def, name, col_ident);
                    }
                }
                AssignmentTarget::Tuple(_) => {
                    // Tuple assignment (col1, col2) = (val1, val2) - not commonly used
                }
            }
            self.expr(&assignment.value);
        }

        if let Some(where_expr) = selection {
            self.expr(where_expr);
        }

        if let Some(def) = table_def {
            self.check_assignments(def, assignments);
        }
    }

    /// Resolve a DELETE statement in the current (statement) frame
    fn delete(&mut self, delete: &Delete) -> Option<Vec<Column>> {
        let tables = match &delete.from {
            sqlparser::ast::FromTable::WithFromKeyword(tables) => tables,
            sqlparser::ast::FromTable::WithoutKeyword(tables) => tables,
        };

        // USING clause first (PostgreSQL / MySQL: DELETE ... USING ...)
        if let Some(using_tables) = &delete.using {
            for table in using_tables {
                self.table_with_joins(table);
            }
        }

        // With MySQL's `DELETE FROM t1 USING t1 JOIN t2`, FROM names aliases of USING tables
        for table in tables {
            let is_using_alias = delete.using.is_some()
                && table.joins.is_empty()
                && matches!(&table.relation, TableFactor::Table { name, alias: None, .. }
                if name.0.len() == 1
                    && self.scope.current_ref().is_some_and(|frame| {
                        lookup_ignore_case(&frame.relations, &name.0[0].value).is_some()
                    }));
            if !is_using_alias {
                self.table_with_joins(table);
            }
        }

        if let Some(where_expr) = &delete.selection {
            self.expr(where_expr);
        }

        self.returning(delete.returning.as_deref())
    }

    // ---------------------------------------------------------------------
    // Queries
    // ---------------------------------------------------------------------

    /// Resolve a query in a new query block. `hides_parent` is set for subqueries in
    /// FROM, which can't see the FROM clause they appear in.
    pub(super) fn query(&mut self, query: &Query, hides_parent: bool) -> Option<Vec<Column>> {
        self.query_with(query, hides_parent, None)
    }

    /// Resolve a query. `recursive_cte` names a recursive CTE (with its explicit
    /// column list) whose columns are those of the anchor (left) branch of the body.
    fn query_with(
        &mut self,
        query: &Query,
        hides_parent: bool,
        recursive_cte: Option<(&str, &[String])>,
    ) -> Option<Vec<Column>> {
        self.scope.push(hides_parent);
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.define_cte(cte, with.recursive);
            }
        }

        let columns = match query.body.as_ref() {
            // ORDER BY / LIMIT see the FROM clause of a plain SELECT
            SetExpr::Select(select) => self.select(select, Some(query)),
            body => {
                let columns = match (body, recursive_cte) {
                    (SetExpr::SetOperation { left, right, .. }, Some((name, names))) => {
                        let left_columns = self.set_expr(left);
                        // The recursive term references the CTE with the anchor's columns
                        self.scope.update_cte(
                            name,
                            Relation::rename_columns(left_columns.clone(), names),
                        );
                        let right_columns = self.set_expr(right);
                        self.set_operation(left_columns, right_columns, right)
                    }
                    (body, _) => self.set_expr(body),
                };
                // ORDER BY / LIMIT of a set operation see its output columns
                self.scope.push(false);
                self.scope.current().relations.insert(
                    String::new(),
                    Relation::new(RelationKind::Subquery, "", columns.clone()),
                );
                self.query_tail(query);
                self.scope.pop();
                columns
            }
        };

        self.scope.pop();
        columns
    }

    /// Register a CTE of the current query
    fn define_cte(&mut self, cte: &sqlparser::ast::Cte, recursive: bool) {
        let name = self.catalog.ident_name(&cte.alias.name);
        let names: Vec<String> = cte
            .alias
            .columns
            .iter()
            .map(|c| c.name.value.clone())
            .collect();
        if recursive {
            // Registered before its body, so the recursive term can reference it
            let columns = Relation::rename_columns(None, &names);
            self.scope.current().ctes.push(Cte {
                name: name.clone(),
                columns,
            });
            let columns = self.query_with(&cte.query, false, Some((&name, &names)));
            self.scope
                .update_cte(&name, Relation::rename_columns(columns, &names));
        } else {
            let columns = self.query(&cte.query, false);
            self.scope.current().ctes.push(Cte {
                name,
                columns: Relation::rename_columns(columns, &names),
            });
        }
    }

    /// Resolve ORDER BY / LIMIT / OFFSET of a query in the current frame
    fn query_tail(&mut self, query: &Query) {
        if let Some(limit) = &query.limit {
            self.expr(limit);
        }
        if let Some(offset) = &query.offset {
            self.expr(&offset.value);
        }
        if let Some(order_by) = &query.order_by {
            for ob in &order_by.exprs {
                self.expr(&ob.expr);
            }
        }
    }

    /// Resolve a set expression (SELECT, UNION, VALUES, ...)
    fn set_expr(&mut self, set_expr: &SetExpr) -> Option<Vec<Column>> {
        match set_expr {
            SetExpr::Select(select) => self.select(select, None),
            SetExpr::Query(query) => self.query(query, false),
            SetExpr::SetOperation { left, right, .. } => {
                let left_columns = self.set_expr(left);
                let right_columns = self.set_expr(right);
                self.set_operation(left_columns, right_columns, right)
            }
            SetExpr::Values(values) => self.values(values),
            SetExpr::Insert(stmt) | SetExpr::Update(stmt) => self.statement(stmt),
            SetExpr::Table(_) => None,
        }
    }

    /// Check the branches of a set operation and combine their output columns
    fn set_operation(
        &mut self,
        left: Option<Vec<Column>>,
        right: Option<Vec<Column>>,
        right_expr: &SetExpr,
    ) -> Option<Vec<Column>> {
        let (Some(mut left), Some(right)) = (left, right) else {
            return None;
        };
        self.check_set_operation(&left, &right, right_expr);
        // Names come from the left branch; a type unknown on the left (e.g. NULL)
        // comes from the right
        if left.len() == right.len() {
            for (l, r) in left.iter_mut().zip(right) {
                if !matches!(l.ty, ExpressionType::Known(_))
                    && matches!(r.ty, ExpressionType::Known(_))
                {
                    l.ty = r.ty;
                }
            }
        }
        Some(left)
    }

    /// Resolve a VALUES list used as a query; its columns are `column1`, `column2`, ...
    fn values(&mut self, values: &Values) -> Option<Vec<Column>> {
        for row in &values.rows {
            for expr in row {
                self.expr(expr);
            }
        }
        let first = values.rows.first()?;
        Some(
            first
                .iter()
                .enumerate()
                .map(|(i, expr)| {
                    Column::new(format!("column{}", i + 1), self.infer_expr_type(expr))
                })
                .collect(),
        )
    }

    /// Resolve a SELECT in a new query block. With `query`, also resolves the query's
    /// ORDER BY / LIMIT, which see the SELECT's FROM clause and output aliases.
    fn select(&mut self, select: &Select, query: Option<&Query>) -> Option<Vec<Column>> {
        self.scope.push(false);

        // FROM first: it builds the scope for every other clause
        for table_with_joins in &select.from {
            self.table_with_joins(table_with_joins);
        }

        // DISTINCT ON (...) expressions
        if let Some(Distinct::On(exprs)) = &select.distinct {
            for expr in exprs {
                self.expr(expr);
            }
        }

        // Named windows: WINDOW w AS (PARTITION BY ... ORDER BY ...)
        for NamedWindowDefinition(_, window) in &select.named_window {
            if let NamedWindowExpr::WindowSpec(spec) = window {
                for e in &spec.partition_by {
                    self.expr(e);
                }
                for ob in &spec.order_by {
                    self.expr(&ob.expr);
                }
            }
        }

        let select_span = Span::from_sqlparser(&select.select_token.0.span);
        for item in &select.projection {
            self.select_item(item, &select_span);
        }

        if let Some(selection) = &select.selection {
            self.expr(selection);
        }

        // GROUP BY can reference output column aliases, like ORDER BY
        let mut aliases = projection_aliases(&select.projection);
        if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
            self.scope.current().select_aliases = aliases;
            for expr in exprs {
                self.expr(expr);
            }
            aliases = std::mem::take(&mut self.scope.current().select_aliases);
        }

        if let Some(having) = &select.having {
            // MySQL and SQLite also resolve output column aliases in HAVING
            // (PostgreSQL doesn't)
            if matches!(self.dialect, SqlDialect::MySQL | SqlDialect::SQLite) {
                self.scope.current().select_aliases = aliases;
                self.expr(having);
                aliases = std::mem::take(&mut self.scope.current().select_aliases);
            } else {
                self.expr(having);
            }
        }

        let columns = self.projection_columns(&select.projection);

        if let Some(query) = query {
            self.scope.current().select_aliases = aliases;
            self.query_tail(query);
        }

        self.scope.pop();
        columns
    }

    /// Output columns of a SELECT list (or RETURNING list) in the current frame, or
    /// `None` if a wildcard can't be expanded
    fn projection_columns(&self, projection: &[SelectItem]) -> Option<Vec<Column>> {
        let frame = self.scope.current_ref()?;
        let mut columns = Vec::new();
        for item in projection {
            match item {
                SelectItem::ExprWithAlias { expr, alias } => {
                    columns.push(Column::new(alias.value.clone(), self.infer_expr_type(expr)));
                }
                SelectItem::UnnamedExpr(expr) => {
                    let name = implicit_column_name(expr).unwrap_or_else(|| UNNAMED_COLUMN.into());
                    columns.push(Column::new(name, self.infer_expr_type(expr)));
                }
                SelectItem::Wildcard(_) => {
                    if frame.relations.is_empty() {
                        return None;
                    }
                    for relation in frame.relations.values() {
                        columns.extend(relation.star_columns()?.cloned());
                    }
                }
                SelectItem::QualifiedWildcard(name, _) => {
                    let qualifier = name.0.last()?;
                    let relation = lookup_ignore_case(&frame.relations, &qualifier.value)?;
                    columns.extend(relation.columns.clone()?);
                }
            }
        }
        Some(columns)
    }

    // ---------------------------------------------------------------------
    // FROM
    // ---------------------------------------------------------------------

    /// Resolve a FROM item with its joins, registering its relations in the current frame
    fn table_with_joins(&mut self, table: &TableWithJoins) {
        self.table_factor(&table.relation);
        for join in &table.joins {
            self.table_factor(&join.relation);
            self.join_constraint(&join.join_operator, &join.relation);
        }
    }

    /// Resolve a JOIN condition (ON / USING / NATURAL)
    fn join_constraint(&mut self, join_op: &JoinOperator, relation: &TableFactor) {
        use JoinOperator::*;
        let constraint = match join_op {
            Inner(c) | LeftOuter(c) | RightOuter(c) | FullOuter(c) | Semi(c) | LeftSemi(c)
            | RightSemi(c) | Anti(c) | LeftAnti(c) | RightAnti(c) => c,
            AsOf { constraint, .. } => constraint,
            CrossJoin | CrossApply | OuterApply => return,
        };

        let right_key = relation_key(relation);
        match constraint {
            JoinConstraint::On(expr) => self.join_on(expr),
            JoinConstraint::Using(columns) => {
                // USING columns exist in both sides by definition and are merged
                // into a single unqualified column, so only check they exist
                for col in columns {
                    let frame = self.scope.current();
                    let (right, left): (Vec<_>, Vec<_>) = frame
                        .relations
                        .iter()
                        .partition(|(k, _)| right_key.as_deref() == Some(k.as_str()));
                    let has = |r: &Relation| {
                        !matches!(r.column(&col.value, self.dialect), ColumnMatch::No)
                    };
                    let in_right = right.is_empty() || right.iter().any(|(_, r)| has(r));
                    let in_left = left.iter().any(|(_, r)| has(r));
                    let name = col.value.to_lowercase();
                    frame.using_columns.insert(name.clone());
                    if let Some(key) = &right_key {
                        if let Some(r) = frame.relations.get_mut(key) {
                            r.merged.insert(name);
                        }
                    }
                    if !(in_right && in_left) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::ColumnNotFound,
                                format!("Column '{}' not found", col.value),
                            )
                            .with_span(Span::from_sqlparser(&col.span)),
                        );
                    }
                }
            }
            JoinConstraint::Natural => {
                // NATURAL JOIN merges every column the two sides have in common
                let Some(key) = right_key else {
                    return;
                };
                let frame = self.scope.current();
                let Some(right) = frame.relations.get(&key) else {
                    return;
                };
                let right_names: Vec<String> =
                    right.column_names().map(|c| c.to_lowercase()).collect();
                let left_names: std::collections::HashSet<String> = frame
                    .relations
                    .iter()
                    .filter(|(k, _)| **k != key)
                    .flat_map(|(_, r)| r.column_names())
                    .map(|c| c.to_lowercase())
                    .collect();
                let common: Vec<String> = right_names
                    .into_iter()
                    .filter(|c| left_names.contains(c))
                    .collect();
                frame.using_columns.extend(common.iter().cloned());
                if let Some(r) = frame.relations.get_mut(&key) {
                    r.merged.extend(common);
                }
            }
            JoinConstraint::None => {}
        }
    }

    /// Register a FROM relation (table, view, CTE, subquery, function) in the current frame
    fn table_factor(&mut self, factor: &TableFactor) {
        match factor {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                let table_name = self.catalog.qualified_name(name);
                let alias_name = alias.as_ref().map(|a| a.name.value.clone());
                let alias_columns: Vec<String> = alias
                    .iter()
                    .flat_map(|a| a.columns.iter().map(|c| c.name.value.clone()))
                    .collect();

                // Table-valued function call (e.g., generate_series(...)): columns are
                // known only from an alias column list
                if args.is_some() {
                    // Without an alias, the function name names the relation
                    if let Some(key) = alias_name.or_else(|| name.0.last().map(|i| i.value.clone()))
                    {
                        let columns = Relation::rename_columns(None, &alias_columns);
                        self.scope.current().relations.insert(
                            key.clone(),
                            Relation::new(RelationKind::Function, key, columns),
                        );
                    }
                    return;
                }

                let key = alias_name.unwrap_or_else(|| table_name.name.clone());
                let cte = table_name
                    .schema
                    .is_none()
                    .then(|| self.scope.cte(self.catalog, &table_name.name))
                    .flatten();
                let mut relation = if let Some(cte) = cte {
                    Relation::new(RelationKind::Cte, cte.name.clone(), cte.columns.clone())
                } else if let Some(table) = Relation::table(self.catalog, &table_name) {
                    table
                } else if let Some(view) = Relation::view(self.catalog, &table_name) {
                    view
                } else {
                    if !is_system_table(self.dialect, &table_name) {
                        let span = name.0.last().map(|id| Span::from_sqlparser(&id.span));
                        let diag = self.table_not_found(&table_name, span);
                        self.diagnostics.push(diag);
                    }
                    // Unknown columns, so references to a missing table don't cascade
                    // into column/alias errors
                    Relation::new(RelationKind::Opaque, table_name.to_string(), None)
                };
                relation.columns = Relation::rename_columns(relation.columns, &alias_columns);
                self.scope.current().relations.insert(key, relation);
            }
            TableFactor::Derived {
                lateral,
                subquery,
                alias,
            } => {
                // Non-LATERAL subqueries can't reference the FROM clause they appear in.
                // LATERAL subqueries can, with lower precedence than their own tables.
                let columns = self.query(subquery, !*lateral);
                let (key, columns) = if let Some(a) = alias {
                    let names: Vec<String> =
                        a.columns.iter().map(|c| c.name.value.clone()).collect();
                    (
                        a.name.value.clone(),
                        Relation::rename_columns(columns, &names),
                    )
                } else {
                    // Unaliased derived table: its columns are still visible unqualified
                    self.unaliased_subqueries += 1;
                    (format!("?subquery{}", self.unaliased_subqueries), columns)
                };
                self.scope.current().relations.insert(
                    key.clone(),
                    Relation::new(RelationKind::Subquery, key, columns),
                );
            }
            TableFactor::TableFunction { alias, .. }
            | TableFactor::Function { alias, .. }
            | TableFactor::UNNEST { alias, .. } => {
                // Table-valued functions (e.g., generate_series, unnest): columns are
                // known only from an alias column list
                let (key, names) = match alias {
                    Some(a) => (
                        a.name.value.clone(),
                        a.columns.iter().map(|c| c.name.value.clone()).collect(),
                    ),
                    None => (format!("{factor}"), Vec::new()),
                };
                let columns = Relation::rename_columns(None, &names);
                self.scope.current().relations.insert(
                    key.clone(),
                    Relation::new(RelationKind::Function, key, columns),
                );
            }
            TableFactor::NestedJoin {
                table_with_joins,
                alias,
            } => {
                let start = self.scope.current().relations.len();
                self.table_with_joins(table_with_joins);
                // `(a JOIN b) AS j`: the alias replaces the joined relations
                if let Some(alias) = alias {
                    let inner = self.scope.current().relations.split_off(start);
                    let columns = inner
                        .values()
                        .map(|r| {
                            r.star_columns()
                                .map(|cols| cols.cloned().collect::<Vec<_>>())
                        })
                        .collect::<Option<Vec<_>>>()
                        .map(|cols| cols.concat());
                    let key = alias.name.value.clone();
                    self.scope.current().relations.insert(
                        key.clone(),
                        Relation::new(RelationKind::Subquery, key, columns),
                    );
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------

    /// Resolve a SELECT item
    fn select_item(&mut self, item: &SelectItem, select_span: &Span) {
        match item {
            SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
                self.expr(expr);
            }
            SelectItem::QualifiedWildcard(name, _) => {
                // table.*
                if let Some(ident) = name.0.last() {
                    let in_scope = self
                        .scope
                        .current_ref()
                        .is_some_and(|f| lookup_ignore_case(&f.relations, &ident.value).is_some());
                    if !in_scope {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::TableNotFound,
                                format!(
                                    "Table or alias '{}' not found in FROM clause",
                                    ident.value
                                ),
                            )
                            .with_span(Span::from_sqlparser(&ident.span)),
                        );
                    }
                }
            }
            SelectItem::Wildcard(_) => {
                // * - valid if we have at least one table
                if self
                    .scope
                    .current_ref()
                    .map_or(true, |f| f.relations.is_empty())
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticKind::TableNotFound,
                            "SELECT * requires at least one table in FROM clause",
                        )
                        .with_span(*select_span),
                    );
                }
            }
        }
    }

    /// Resolve a subquery in an expression (it sees the enclosing query blocks)
    fn subquery(&mut self, query: &Query) -> Option<Vec<Column>> {
        let columns = self.query(query, false);
        self.subquery_columns
            .insert(query as *const Query as usize, columns.clone());
        columns
    }

    /// Resolve an expression and check its types
    pub(super) fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Identifier(ident) => self.column(None, ident),
            Expr::CompoundIdentifier(idents) => match idents.as_slice() {
                // table.column or schema.table.column
                [table, column] | [_, table, column] => self.column(Some(table), column),
                _ => {}
            },
            Expr::BinaryOp { .. } => {
                // Binary operators are left-associative, so a long chain
                // (`a = 1 OR a = 2 OR ...`) nests as deep as it is long: walk its
                // left spine in a loop instead of recursing, in the same order
                // (left operand, right operand, then the operator's check)
                let mut chain = Vec::new();
                let mut leftmost = expr;
                while let Expr::BinaryOp { left, op, right } = leftmost {
                    chain.push((&**left, op, &**right));
                    leftmost = left;
                }
                self.expr(leftmost);
                // The type of the left operand, passed up an arithmetic chain
                let mut left_type = None;
                for (left, op, right) in chain.into_iter().rev() {
                    self.expr(right);
                    left_type = self.check_binary_op(left, op, right, left_type);
                }
            }
            Expr::UnaryOp { expr, .. }
            | Expr::Nested(expr)
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::IsTrue(expr)
            | Expr::IsFalse(expr)
            | Expr::IsNotTrue(expr)
            | Expr::IsNotFalse(expr)
            | Expr::IsUnknown(expr)
            | Expr::IsNotUnknown(expr)
            | Expr::Cast { expr, .. }
            | Expr::Extract { expr, .. }
            | Expr::Collate { expr, .. }
            | Expr::Ceil { expr, .. }
            | Expr::Floor { expr, .. } => self.expr(expr),
            Expr::Function(func) => {
                self.function_args(&func.args);
                // ORDER BY inside aggregate arguments: array_agg(x ORDER BY y)
                if let sqlparser::ast::FunctionArguments::List(list) = &func.args {
                    for clause in &list.clauses {
                        if let sqlparser::ast::FunctionArgumentClause::OrderBy(order_by) = clause {
                            for ob in order_by {
                                self.expr(&ob.expr);
                            }
                        }
                    }
                }
                // WITHIN GROUP (ORDER BY ...)
                for ob in &func.within_group {
                    self.expr(&ob.expr);
                }
                // FILTER (WHERE ...)
                if let Some(filter) = &func.filter {
                    self.expr(filter);
                }
                // OVER (PARTITION BY ... ORDER BY ...)
                if let Some(sqlparser::ast::WindowType::WindowSpec(spec)) = &func.over {
                    for e in &spec.partition_by {
                        self.expr(e);
                    }
                    for ob in &spec.order_by {
                        self.expr(&ob.expr);
                    }
                }
            }
            Expr::InList { expr, list, .. } => {
                self.expr(expr);
                for e in list {
                    self.expr(e);
                }
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.expr(expr);
                let columns = self.subquery(subquery);
                self.check_in_subquery(expr, columns.as_deref());
            }
            Expr::Subquery(query) => {
                self.subquery(query);
            }
            Expr::Exists { subquery, .. } => {
                self.subquery(subquery);
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.expr(expr);
                self.expr(low);
                self.expr(high);
            }
            Expr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                if let Some(op) = operand {
                    self.expr(op);
                }
                for cond in conditions {
                    self.expr(cond);
                }
                for result in results {
                    self.expr(result);
                }
                if let Some(else_r) = else_result {
                    self.expr(else_r);
                }
                let branches: Vec<&Expr> = results.iter().chain(else_result.as_deref()).collect();
                self.check_case_branches(&branches);
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => {
                self.expr(expr);
                if let Some(from) = substring_from {
                    self.expr(from);
                }
                if let Some(for_expr) = substring_for {
                    self.expr(for_expr);
                }
            }
            Expr::Trim {
                expr, trim_what, ..
            } => {
                self.expr(expr);
                if let Some(what) = trim_what {
                    self.expr(what);
                }
            }
            Expr::Position { expr, r#in } => {
                self.expr(expr);
                self.expr(r#in);
            }
            Expr::Like { expr, pattern, .. }
            | Expr::ILike { expr, pattern, .. }
            | Expr::SimilarTo { expr, pattern, .. }
            | Expr::RLike { expr, pattern, .. } => {
                self.expr(expr);
                self.expr(pattern);
            }
            Expr::JsonAccess { value, .. } => self.expr(value),
            Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => {
                self.expr(timestamp);
                self.expr(time_zone);
            }
            Expr::Overlay {
                expr,
                overlay_what,
                overlay_from,
                overlay_for,
            } => {
                self.expr(expr);
                self.expr(overlay_what);
                self.expr(overlay_from);
                if let Some(for_expr) = overlay_for {
                    self.expr(for_expr);
                }
            }
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Tuple(exprs) => {
                for e in exprs {
                    self.expr(e);
                }
            }
            Expr::Array(arr) => {
                for e in &arr.elem {
                    self.expr(e);
                }
            }
            Expr::Subscript { expr, subscript } => {
                self.expr(expr);
                match subscript.as_ref() {
                    Subscript::Index { index } => self.expr(index),
                    Subscript::Slice {
                        lower_bound,
                        upper_bound,
                        stride,
                    } => {
                        for e in [lower_bound, upper_bound, stride].into_iter().flatten() {
                            self.expr(e);
                        }
                    }
                }
            }
            Expr::Method(method) => {
                self.expr(&method.expr);
                for func in &method.method_chain {
                    self.function_args(&func.args);
                }
            }
            Expr::GroupingSets(sets) | Expr::Cube(sets) | Expr::Rollup(sets) => {
                for e in sets.iter().flatten() {
                    self.expr(e);
                }
            }
            // Literals, intervals, and other expressions don't need column resolution
            _ => {}
        }
    }

    /// Resolve a JOIN ... ON condition: comparisons joined by AND/OR are checked as
    /// join conditions (E0007), everything else as ordinary expressions
    fn join_on(&mut self, expr: &Expr) {
        match expr {
            Expr::Nested(inner) => self.join_on(inner),
            Expr::BinaryOp { left, op, right } if is_logical_operator(op) => {
                self.join_on(left);
                self.join_on(right);
            }
            Expr::BinaryOp { left, op, right } if is_comparison_operator(op) => {
                self.expr(left);
                self.expr(right);
                self.check_join_comparison(left, right);
            }
            other => self.expr(other),
        }
    }

    /// Resolve function arguments (handles Named, ExprNamed, and Unnamed variants)
    fn function_args(&mut self, args: &sqlparser::ast::FunctionArguments) {
        use sqlparser::ast::{FunctionArg, FunctionArgExpr, FunctionArguments};
        if let FunctionArguments::List(arg_list) = args {
            for arg in &arg_list.args {
                match arg {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(e),
                        ..
                    }
                    | FunctionArg::ExprNamed {
                        arg: FunctionArgExpr::Expr(e),
                        ..
                    } => self.expr(e),
                    _ => {}
                }
            }
        }
    }

    /// Resolve a column reference
    fn column(&mut self, table_ident: Option<&Ident>, column_ident: &Ident) {
        let column_name = &column_ident.value;
        let column_span = Span::from_sqlparser(&column_ident.span);

        // Qualified column reference (table.column)
        if let Some(table_id) = table_ident {
            let Some(relation) = self.scope.relation(&table_id.value) else {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TableNotFound,
                        format!(
                            "Table or alias '{}' not found in FROM clause",
                            table_id.value
                        ),
                    )
                    .with_span(Span::from_sqlparser(&table_id.span)),
                );
                return;
            };
            if !matches!(relation.column(column_name, self.dialect), ColumnMatch::No) {
                return;
            }
            let mut diag = Diagnostic::error(
                DiagnosticKind::ColumnNotFound,
                format!(
                    "Column '{}' not found in {} '{}'",
                    column_name,
                    relation.kind_name(),
                    relation.name
                ),
            )
            .with_span(column_span);
            if let Some(help) = missing_column_help(
                relation.former_column(column_name),
                relation.column_names(),
                column_name,
            ) {
                diag = diag.with_help(help);
            }
            self.diagnostics.push(diag);
            return;
        }

        // Unqualified column reference: the innermost query block that has it wins
        match self.scope.column(column_name, self.dialect) {
            ColumnLookup::Found(_) | ColumnLookup::Unknown => {}
            ColumnLookup::Ambiguous(tables) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::AmbiguousColumn,
                        format!(
                            "Column '{}' is ambiguous (found in tables: {})",
                            column_name,
                            tables.join(", ")
                        ),
                    )
                    .with_span(column_span)
                    .with_help(format!(
                        "Qualify the column with a table name: {}.{}",
                        tables[0], column_name
                    )),
                );
            }
            ColumnLookup::NotFound => {
                // An output column alias (valid in ORDER BY / GROUP BY)
                let is_alias = self.scope.current_ref().is_some_and(|f| {
                    f.select_aliases
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(column_name))
                });
                // Whole-row reference to a table (`json_agg(u)`, `row_to_json(t)`)
                let is_relation = self.scope.relation(column_name).is_some();
                // Keywords and variables the parser represents as identifiers:
                // DEFAULT, date/time units (`TIMESTAMPDIFF(DAY, ...)`), `@var`
                let is_keyword = column_ident.quote_style.is_none()
                    && (column_name.eq_ignore_ascii_case("DEFAULT")
                        || is_date_part_keyword(column_name)
                        || column_name.starts_with('@'));
                if is_alias || is_relation || is_keyword {
                    return;
                }

                let relations: Vec<&Relation> = self
                    .scope
                    .current_ref()
                    .map(|f| f.relations.values().collect())
                    .unwrap_or_default();
                // Name the relation that was searched when there is only one in reach
                let mut visible = self.scope.visible().flat_map(|f| f.relations.values());
                let message = match (visible.next(), visible.next()) {
                    (Some(only), None) => format!(
                        "Column '{}' not found in {} '{}'",
                        column_name,
                        only.kind_name(),
                        only.name
                    ),
                    _ => format!("Column '{column_name}' not found"),
                };
                let former = relations.iter().find_map(|r| r.former_column(column_name));
                let candidates = relations.iter().flat_map(|r| r.column_names());
                let mut diag = Diagnostic::error(DiagnosticKind::ColumnNotFound, message)
                    .with_span(column_span);
                if let Some(help) = missing_column_help(former, candidates, column_name) {
                    diag = diag.with_help(help);
                }
                self.diagnostics.push(diag);
            }
        }
    }

    /// Whether the database supplies a value for `col` when an INSERT omits it
    fn column_has_implicit_value(&self, table: &TableDef, col: &ColumnDef) -> bool {
        if col.nullable || col.default.is_some() || col.identity.is_some() || col.auto_increment {
            return true;
        }
        // SQLite: a single-column INTEGER PRIMARY KEY is an alias for the rowid
        self.dialect == SqlDialect::SQLite
            && col.data_type.is_integer()
            && table.primary_key.as_ref().map_or(col.is_primary_key, |pk| {
                pk.columns.len() == 1 && pk.columns[0].eq_ignore_ascii_case(&col.name)
            })
    }

    /// Build a "table not found" diagnostic, suggesting a similarly named table, view or
    /// CTE, or else a likely reason why the table is missing
    fn table_not_found(&self, table_name: &QualifiedName, span: Option<Span>) -> Diagnostic {
        let help = match (
            self.dropped_table_hint(table_name),
            self.skipped_definition_hint(table_name),
            self.similar_table(table_name),
        ) {
            (Some(hint), _, _) => hint,
            (None, Some(hint), _) => hint,
            (None, None, Some(suggestion)) => format!("Did you mean '{suggestion}'?"),
            (None, None, None) => self.missing_table_hint(table_name),
        };
        let mut diag = Diagnostic::error(
            DiagnosticKind::TableNotFound,
            format!("Table '{table_name}' not found"),
        )
        .with_help(help);
        if let Some(span) = span {
            diag = diag.with_span(span);
        }
        diag
    }

    /// The table, view or CTE most similar to a missing one, named the way a query
    /// refers to it: schema-qualified unless it is in the default schema
    fn similar_table(&self, table_name: &QualifiedName) -> Option<String> {
        let catalog = self.catalog;
        let typed = table_name.to_string();
        let schema = table_name
            .schema
            .as_deref()
            .unwrap_or(&catalog.default_schema);
        // Tables in the schema that was searched win ties with those in other schemas
        let (same_schema, other_schemas): (Vec<_>, Vec<_>) =
            catalog.table_or_view_names().into_iter().partition(|n| {
                n.schema
                    .as_deref()
                    .is_some_and(|s| catalog.names_match(s, schema))
            });
        let display = |n: QualifiedName| match &n.schema {
            Some(s) if !catalog.names_match(s, &catalog.default_schema) => (n.to_string(), n.name),
            _ => (n.name.clone(), n.name),
        };
        // CTEs can only be referred to unqualified
        let ctes = if table_name.schema.is_none() {
            self.scope.cte_names()
        } else {
            Vec::new()
        };
        let ctes = ctes.into_iter().map(|n| (n.clone(), n));
        let candidates = same_schema
            .into_iter()
            .map(display)
            .chain(ctes)
            .chain(other_schemas.into_iter().map(display))
            // Never suggest what was written
            .filter(|(shown, _)| !catalog.names_match(shown, &typed));
        find_most_similar(candidates, |(_, name)| name.as_str(), &table_name.name)
            .map(|(shown, _)| shown)
    }

    /// Why a table is missing when an earlier statement of the query file dropped it
    fn dropped_table_hint(&self, table_name: &QualifiedName) -> Option<String> {
        let dropped = self
            .catalog
            .dropped_relations
            .iter()
            .rev()
            .find(|d| self.catalog.relation_names_match(&d.name, table_name))?;
        Some(format!(
            "'{table_name}' was dropped at line {} of this file",
            dropped.line
        ))
    }

    /// Why a table is missing when an earlier statement of the query file that
    /// defines it could not be parsed
    fn skipped_definition_hint(&self, table_name: &QualifiedName) -> Option<String> {
        let def = self.catalog.skipped_definitions.iter().rev().find(|d| {
            d.line.is_some()
                && d.name.as_deref().is_some_and(|name| {
                    let last = name.rsplit('.').next().unwrap_or(name);
                    last.trim_matches('"')
                        .eq_ignore_ascii_case(&table_name.name)
                })
        })?;
        Some(format!(
            "The {} statement for '{}' on line {} could not be parsed (see the parse error there), so the table is unknown",
            def.kind,
            def.name.as_deref().unwrap_or_default(),
            def.line.unwrap_or_default()
        ))
    }

    /// Why a table that has no similarly named one may be missing
    fn missing_table_hint(&self, table_name: &QualifiedName) -> String {
        const LIST_TABLES: &str =
            "run `sqlsift schema <schema files>` to list the tables that were loaded";
        let skipped = &self.catalog.skipped_definitions;
        let defines_table = |name: &str| {
            let unquote = |part: &str| {
                part.trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
                    .to_string()
            };
            name.rsplit('.')
                .next()
                .is_some_and(|last| unquote(last).eq_ignore_ascii_case(&table_name.name))
        };
        if let Some(def) = skipped
            .iter()
            .find(|d| d.name.as_deref().is_some_and(defines_table))
        {
            return format!(
                "A {} statement for '{}' in the schema could not be parsed and was skipped (see the schema warnings)",
                def.kind,
                def.name.as_deref().unwrap_or_default()
            );
        }
        if let Some(schema) = &table_name.schema {
            let schema_known = self
                .catalog
                .schemas
                .keys()
                .any(|s| self.catalog.names_match(s, schema));
            if !schema_known {
                return format!(
                    "Schema '{schema}' has no tables in the schema input; check that the schema files that define it are included"
                );
            }
        }
        if !skipped.is_empty() {
            return format!(
                "{} schema statement(s) could not be parsed and were skipped (see the schema warnings), so the table may be defined in one of them; {}",
                skipped.len(),
                LIST_TABLES
            );
        }
        format!("Check that the table exists in your schema definition; {LIST_TABLES}")
    }
}

/// Whether an operator is a comparison (=, <>, <, <=, >, >=)
pub(super) fn is_comparison_operator(op: &sqlparser::ast::BinaryOperator) -> bool {
    use sqlparser::ast::BinaryOperator::*;
    matches!(op, Eq | NotEq | Lt | LtEq | Gt | GtEq)
}

/// Whether an operator is AND / OR
fn is_logical_operator(op: &sqlparser::ast::BinaryOperator) -> bool {
    use sqlparser::ast::BinaryOperator::*;
    matches!(op, And | Or)
}

/// Scope key (alias or table name) a FROM relation is registered under
fn relation_key(factor: &TableFactor) -> Option<String> {
    match factor {
        TableFactor::Table { name, alias, .. } => alias
            .as_ref()
            .map(|a| a.name.value.clone())
            .or_else(|| name.0.last().map(|i| i.value.clone())),
        TableFactor::Derived { alias, .. } => alias.as_ref().map(|a| a.name.value.clone()),
        _ => None,
    }
}

/// Output column names of a SELECT list usable as aliases in ORDER BY / GROUP BY
fn projection_aliases(projection: &[SelectItem]) -> Vec<String> {
    projection
        .iter()
        .filter_map(|item| match item {
            SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
            // Column name also acts as implicit alias
            SelectItem::UnnamedExpr(Expr::Identifier(ident)) => Some(ident.value.clone()),
            _ => None,
        })
        .collect()
}

/// System catalog tables, whose columns sqlsift doesn't model
fn is_system_table(dialect: SqlDialect, name: &QualifiedName) -> bool {
    let schema = name.schema.as_deref().map(str::to_ascii_lowercase);
    let table = name.name.to_ascii_lowercase();
    match dialect {
        SqlDialect::PostgreSQL => {
            matches!(schema.as_deref(), Some("pg_catalog" | "information_schema"))
                || (schema.is_none() && table.starts_with("pg_"))
        }
        SqlDialect::MySQL => matches!(
            schema.as_deref(),
            Some("information_schema" | "mysql" | "performance_schema" | "sys")
        ),
        SqlDialect::SQLite => table.starts_with("sqlite_"),
    }
}

/// Date/time unit keywords that parsers represent as identifiers in function
/// arguments (`TIMESTAMPDIFF(DAY, a, b)`, `EXTRACT`-like functions)
fn is_date_part_keyword(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "MICROSECOND"
            | "MILLISECOND"
            | "SECOND"
            | "MINUTE"
            | "HOUR"
            | "DAY"
            | "WEEK"
            | "MONTH"
            | "QUARTER"
            | "YEAR"
            | "SECOND_MICROSECOND"
            | "MINUTE_MICROSECOND"
            | "MINUTE_SECOND"
            | "HOUR_MICROSECOND"
            | "HOUR_SECOND"
            | "HOUR_MINUTE"
            | "DAY_MICROSECOND"
            | "DAY_SECOND"
            | "DAY_MINUTE"
            | "DAY_HOUR"
            | "YEAR_MONTH"
    )
}

/// Name PostgreSQL gives an unaliased SELECT expression (`count(*)` -> `count`,
/// `t.col` -> `col`, `col::int` -> `col`), or `None` for `?column?`
fn implicit_column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(ident) => Some(ident.value.clone()),
        Expr::CompoundIdentifier(idents) => idents.last().map(|i| i.value.clone()),
        Expr::Function(func) => func.name.0.last().map(|i| i.value.to_lowercase()),
        Expr::Cast { expr, .. } | Expr::Nested(expr) => implicit_column_name(expr),
        Expr::Case { .. } => Some("case".to_string()),
        _ => None,
    }
}

/// Help for a column of a catalog table that doesn't exist
fn missing_table_column_help(table: &TableDef, name: &str) -> Option<String> {
    missing_column_help(
        table.former_column(name),
        table.columns.keys().cloned(),
        name,
    )
}

/// Help for a column that doesn't exist: what became of it if ALTER TABLE renamed or
/// dropped it (a rename is rarely a small edit, so "did you mean" can't find it),
/// else the most similar existing column
fn missing_column_help(
    former: Option<&FormerColumn>,
    candidates: impl IntoIterator<Item = String>,
    name: &str,
) -> Option<String> {
    match former {
        Some(FormerColumn {
            renamed_to: Some(new_name),
            ..
        }) => Some(format!(
            "'{name}' was renamed to '{new_name}' by ALTER TABLE in the schema"
        )),
        Some(FormerColumn {
            renamed_to: None, ..
        }) => Some(format!("'{name}' was dropped by ALTER TABLE in the schema")),
        None => find_similar_name(candidates, name).map(|s| format!("Did you mean '{s}'?")),
    }
}
