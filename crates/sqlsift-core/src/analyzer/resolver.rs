//! Statement resolver - resolves table and column references and checks types
//!
//! A single walk over each statement: FROM clauses build the [`Scope`], names are
//! resolved against it, and every query block reports its output columns with their
//! types, so CTEs, subqueries and views carry column types into the queries that use
//! them. The type checks themselves live in `type_check.rs`.

use sqlparser::ast::{
    Assignment, AssignmentTarget, ConflictTarget, Delete, Distinct, ExcludeSelectItem, Expr,
    FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr, Ident, Insert, JoinConstraint,
    JoinOperator, NamedWindowDefinition, NamedWindowExpr, ObjectName, OnConflictAction, OnInsert,
    Query, RenameSelectItem, Select, SelectItem, SetExpr, Statement, Subscript, TableAlias,
    TableFactor, TableWithJoins, Values, WildcardAdditionalOptions,
};
use std::collections::HashMap;

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{
    split_dotted_idents, Catalog, ColumnDef, FormerColumn, QualifiedName, TableDef,
};
use crate::suggest::{find_most_similar, find_similar_name};
use crate::types::SqlType;

use super::scope::{
    lookup_ignore_case, split_column_ref, Column, ColumnLookup, ColumnMatch, Cte, ExpressionType,
    Relation, RelationKind, Scope, UNNAMED_COLUMN,
};
use super::structure::Clause;

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
    /// Script variables declared by earlier statements of the file (`DECLARE x`):
    /// names that are not columns may be these
    variables: Vec<String>,
    /// Whether a name at this location is made from a template tag (`AS {{ c }}`)
    templated_name: Option<&'a dyn Fn(&Span) -> bool>,
    /// Relation names the FROM clause being resolved gives twice (E0009)
    pub(super) duplicate_names: Vec<String>,
    /// The statement has template tags, which may expand to columns or clauses
    pub(super) templated_statement: bool,
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
            variables: Vec::new(),
            templated_name: None,
            duplicate_names: Vec::new(),
            templated_statement: false,
        }
    }

    /// Script variables declared by earlier statements (`DECLARE x INT64`)
    pub(super) fn with_variables(mut self, variables: &[String]) -> Self {
        self.variables = variables.to_vec();
        self
    }

    /// Names at the locations `templated_name` accepts are made from template tags:
    /// an output column with such an alias (`AS {{ c }}`) has an unknown name
    pub(super) fn with_templated_names(
        mut self,
        templated_name: &'a dyn Fn(&Span) -> bool,
    ) -> Self {
        self.templated_name = Some(templated_name);
        self
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

        // BigQuery: a table of a project the schema doesn't describe is unknown
        if self.dialect == SqlDialect::BigQuery
            && !self.catalog.database_is_known(&insert.table_name)
        {
            if let Some(source) = &insert.source {
                self.insert_source(source);
            }
            return None;
        }

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
            } else if !(self.dialect == SqlDialect::BigQuery
                && self.bigquery_table_may_exist(&table_name))
            {
                let diag = self.table_not_found(&table_name, table_span);
                self.diagnostics.push(diag);
            }
            return None;
        };

        self.check_duplicate_insert_columns(&insert.columns);

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

        // Without a column list, values are positional (SQLite skips generated columns)
        let positional_columns = self.positional_columns(table_def);
        let expected_count = if insert.columns.is_empty() {
            positional_columns.len()
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

        // PostgreSQL: without a column list, VALUES may give the first N columns
        // only; the rest get their defaults
        let fills_trailing =
            insert.columns.is_empty() && self.dialect.insert_fills_trailing_columns();
        // Without a column list, the columns a PostgreSQL VALUES list gives
        let positional: Option<Vec<String>> = match (fills_trailing, values) {
            (true, Some(rows)) => rows
                .iter()
                .map(Vec::len)
                .max()
                .filter(|&n| n <= expected_count)
                .map(|n| {
                    positional_columns
                        .iter()
                        .take(n)
                        .map(|c| (*c).to_string())
                        .collect()
                }),
            _ => None,
        };

        // NOT NULL columns without a default must be given a value. Only checked
        // with an explicit column list (or DEFAULT VALUES, or a PostgreSQL VALUES
        // list without one); otherwise every column is positional and E0005 covers
        // missing values.
        // Skipped when the column list or VALUES arity is already wrong: the missing
        // column is then usually the one that was misspelled or miscounted.
        // (VALUES literals carry no source location, so point at the table name.)
        let default_values = insert.source.is_none() && insert.columns.is_empty();
        let given: Vec<String> = positional
            .clone()
            .unwrap_or_else(|| insert.columns.iter().map(|c| c.value.clone()).collect());
        let columns_valid = given.iter().all(|c| table_def.column_exists(c));
        let arity_valid = match (values, &selected) {
            (Some(rows), _) => rows.iter().all(|row| row.len() == given.len()),
            (None, Some(selected)) => selected.len() == given.len(),
            (None, None) => true,
        };
        if (!given.is_empty() || default_values) && columns_valid && arity_valid {
            self.check_required_columns(&given, table_def, &table_name, table_span);
        }

        if let Some(rows) = values {
            for row in rows {
                if let Some(given) = &positional {
                    // Shorter rows than the longest one (PostgreSQL rejects lists of
                    // different lengths); fewer values than columns are fine
                    if row.len() != given.len() {
                        let mut diag = Diagnostic::error(
                            DiagnosticKind::ColumnCountMismatch,
                            format!(
                                "VALUES row has {} value(s) but another row has {}",
                                row.len(),
                                given.len()
                            ),
                        )
                        .with_help("All VALUES rows must have the same number of values");
                        diag.span = table_span;
                        self.diagnostics.push(diag);
                    }
                } else if row.len() != expected_count {
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
                    self.check_aggregates_in(expr, Clause::Values);
                }
            }
            self.check_insert_values(insert, table_def, rows);
            self.check_generated_insert(insert, table_def, Some(rows));
        } else if let Some(selected) = &selected {
            self.check_generated_insert(insert, table_def, None);
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

    /// Report NOT NULL columns without a default that an INSERT omits (E0008);
    /// `given` are the columns the INSERT gives values for
    fn check_required_columns(
        &mut self,
        given: &[String],
        table_def: &TableDef,
        table_name: &QualifiedName,
        span: Option<Span>,
    ) {
        let missing: Vec<&str> = table_def
            .columns
            .values()
            .filter(|col| {
                !given.iter().any(|c| c.eq_ignore_ascii_case(&col.name))
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
                    self.check_conflict_target(table_def, table_name, columns);
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
        self.check_duplicate_assignments(assignments);
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
            TableFactor::Table { name, .. }
                if self.dialect != SqlDialect::BigQuery || self.catalog.database_is_known(name) =>
            {
                Some(self.catalog.qualified_name(name))
            }
            _ => None,
        };
        let table_def = table_name.as_ref().and_then(|n| self.catalog.get_table(n));

        for assignment in assignments {
            match &assignment.target {
                AssignmentTarget::ColumnName(col_name) if col_name.0.len() >= 2 => {
                    // `SET alias.col = ...` (MySQL multi-table UPDATE)
                    let parts = split_dotted_idents(&col_name.0);
                    if let Some((schema, table, column)) = split_column_ref(&parts) {
                        self.column(Some((schema, table)), column);
                    }
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
            self.check_aggregates_in(&assignment.value, Clause::UpdateSet);
        }
        self.check_duplicate_assignments(assignments);

        if let Some(where_expr) = selection {
            self.expr(where_expr);
            self.check_aggregates_in(where_expr, Clause::Where);
        }

        if let Some(def) = table_def {
            self.check_assignments(def, assignments);
            self.check_generated_update(def, assignments);
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
            self.check_aggregates_in(where_expr, Clause::Where);
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
        self.check_duplicate_cte_names(query);
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                // Snowflake: a CTE is recursive when it references itself, with or
                // without the RECURSIVE keyword
                let recursive = with.recursive
                    || (self.dialect == SqlDialect::Snowflake && self.references_itself(cte));
                self.define_cte(cte, recursive);
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
                if let Some(columns) = &columns {
                    self.check_order_by_positions(query, columns.len(), None);
                }
                columns
            }
        };

        self.scope.pop();
        columns
    }

    /// Whether the body of `cte` reads a relation with the CTE's (unqualified) name
    fn references_itself(&self, cte: &sqlparser::ast::Cte) -> bool {
        let name = self.catalog.ident_name(&cte.alias.name);
        let found =
            sqlparser::ast::visit_relations(&cte.query, |relation| match relation.0.as_slice() {
                [ident] if self.catalog.ident_name(ident) == name => {
                    std::ops::ControlFlow::Break(())
                }
                _ => std::ops::ControlFlow::Continue(()),
            });
        found.is_break()
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
            self.check_alias_count(
                "WITH query",
                &cte.alias.name,
                columns.as_deref(),
                names.len(),
                true,
            );
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
        let duplicates = self.check_duplicate_relation_names(&select.from);
        let outer_duplicates = std::mem::replace(&mut self.duplicate_names, duplicates);
        for table_with_joins in &select.from {
            self.table_with_joins(table_with_joins);
        }
        self.duplicate_names = outer_duplicates;

        // Hive / Databricks `LATERAL VIEW [OUTER] explode(expr) alias AS c1, ...`: the
        // generator's argument sees the FROM clause, and its output is a relation with
        // the column aliases (of unknown types; any columns without aliases)
        for (i, view) in select.lateral_views.iter().enumerate() {
            self.expr(&view.lateral_view);
            let key = view
                .lateral_view_name
                .0
                .last()
                .map_or_else(|| format!("?lateral_view{i}"), |n| n.value.clone());
            let names: Vec<String> = view
                .lateral_col_alias
                .iter()
                .map(|c| c.value.clone())
                .collect();
            let columns = Relation::rename_columns(None, &names);
            self.scope.current().relations.insert(
                key.clone(),
                Relation::new(RelationKind::Function, key, columns),
            );
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

        // Output column aliases. Snowflake, Redshift and Databricks also resolve
        // them in WHERE and in later items of the SELECT list (lateral column
        // aliases); an alias is only used for a name no table in scope has.
        // Only explicit aliases of earlier items are lateral aliases: `SELECT nope`
        // must not resolve `nope` as its own alias.
        let mut aliases = projection_aliases(&select.projection, self.dialect);
        let lateral = self.dialect.lateral_column_aliases();

        let select_span = Span::from_sqlparser(&select.select_token.0.span);
        for item in &select.projection {
            self.select_item(item, &select_span);
            if let SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } = item {
                self.check_nested_aggregates(expr);
            }
            if let (true, SelectItem::ExprWithAlias { alias, .. }) = (lateral, item) {
                self.scope
                    .current()
                    .select_aliases
                    .push(alias.value.clone());
            }
        }

        if let Some(selection) = &select.selection {
            self.expr(selection);
            self.check_aggregates_in(selection, Clause::Where);
        }
        if lateral {
            self.scope.current().select_aliases.clear();
        }

        // GROUP BY can reference output column aliases, like ORDER BY
        if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
            self.scope.current().select_aliases = aliases;
            for expr in exprs {
                self.expr(expr);
                self.check_aggregates_in(expr, Clause::GroupBy);
            }
            aliases = std::mem::take(&mut self.scope.current().select_aliases);
        }

        if let Some(having) = &select.having {
            self.check_aggregates_in(having, Clause::Having);
            // Every dialect but PostgreSQL also resolves output column aliases in HAVING
            if self.dialect.aliases_in_having() {
                self.scope.current().select_aliases = aliases;
                self.expr(having);
                aliases = std::mem::take(&mut self.scope.current().select_aliases);
            } else {
                self.expr(having);
            }
        }

        // QUALIFY (Snowflake, BigQuery, Databricks) filters on window functions and
        // can reference output column aliases
        if let Some(qualify) = &select.qualify {
            self.scope.current().select_aliases = aliases;
            self.expr(qualify);
            aliases = std::mem::take(&mut self.scope.current().select_aliases);
        }

        let columns = self.projection_columns(&select.projection);
        if let Some(columns) = &columns {
            self.check_group_by_positions(select, columns.len());
        }

        if let Some(query) = query {
            self.scope.current().select_aliases = aliases;
            self.query_tail(query);
            self.check_distinct_order_by(select, query);
            if let Some(columns) = &columns {
                self.check_order_by_positions(query, columns.len(), Some(select_span));
            }
        }

        self.check_grouping(select, query);
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
                    let templated = self
                        .templated_name
                        .is_some_and(|f| f(&Span::from_sqlparser(&alias.span)));
                    let name = if templated {
                        UNNAMED_COLUMN.to_string()
                    } else {
                        alias.value.clone()
                    };
                    columns.push(Column::new(name, self.infer_expr_type(expr)));
                }
                // BigQuery `f(x).*`: the fields of a STRUCT, which aren't modeled
                SelectItem::UnnamedExpr(Expr::CompositeAccess { key, .. })
                    if key.value == super::bigquery::STRUCT_STAR_FIELD =>
                {
                    return None;
                }
                SelectItem::UnnamedExpr(expr) => {
                    let name = implicit_column_name(expr).unwrap_or_else(|| UNNAMED_COLUMN.into());
                    let mut column = Column::new(name, self.infer_expr_type(expr));
                    // Only a column reference is named the same in every dialect
                    column.guessed_name =
                        !matches!(expr, Expr::Identifier(_) | Expr::CompoundIdentifier(_));
                    columns.push(column);
                }
                SelectItem::Wildcard(options) => {
                    if frame.relations.is_empty() {
                        return None;
                    }
                    let mut star: Vec<Column> = Vec::new();
                    for relation in frame.relations.values() {
                        for column in relation.star_columns()? {
                            // A column merged by USING / NATURAL JOIN appears once
                            // (also when the merge couldn't be tied to a relation)
                            let merged = frame.using_columns.contains(&column.name.to_lowercase())
                                && star
                                    .iter()
                                    .any(|c| c.name.eq_ignore_ascii_case(&column.name));
                            if !merged {
                                star.push(column.clone());
                            }
                        }
                    }
                    columns.extend(self.apply_wildcard_options(star, options)?);
                }
                SelectItem::QualifiedWildcard(name, options) => {
                    let parts = split_dotted_idents(&name.0);
                    let [.., table] = parts.as_ref() else {
                        return None;
                    };
                    let schema = parts.len().checked_sub(2).map(|i| &parts[i]);
                    let relation = frame.qualified_relation(self.catalog, schema, table)?;
                    columns
                        .extend(self.apply_wildcard_options(relation.columns.clone()?, options)?);
                }
            }
        }
        Some(columns)
    }

    /// Columns of `SELECT *` after its `EXCLUDE` / `EXCEPT`, `RENAME` and `REPLACE`
    /// options, or `None` if they can't be determined (`ILIKE`)
    fn apply_wildcard_options(
        &self,
        mut columns: Vec<Column>,
        options: &WildcardAdditionalOptions,
    ) -> Option<Vec<Column>> {
        if options.opt_ilike.is_some() {
            return None;
        }
        let excluded = wildcard_excluded(options);
        columns.retain(|c| {
            !excluded
                .iter()
                .any(|e| e.value.eq_ignore_ascii_case(&c.name))
        });
        if let Some(replace) = &options.opt_replace {
            for element in &replace.items {
                let name = &element.column_name.value;
                if let Some(col) = columns
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(name))
                {
                    col.ty = self.infer_expr_type(&element.expr);
                }
            }
        }
        for rename in wildcard_renamed(options) {
            if let Some(col) = columns
                .iter_mut()
                .find(|c| c.name.eq_ignore_ascii_case(&rename.ident.value))
            {
                col.rename(&rename.alias.value);
            }
        }
        Some(columns)
    }

    /// Resolve the column names and expressions in the options of `SELECT *` /
    /// `t.*` (`EXCLUDE (a)`, `RENAME (a AS b)`, `REPLACE (expr AS a)`)
    fn wildcard_options(
        &mut self,
        qualifier: Option<(Option<&Ident>, &Ident)>,
        options: &WildcardAdditionalOptions,
    ) {
        if let Some(replace) = &options.opt_replace {
            for element in &replace.items {
                self.expr(&element.expr);
            }
        }
        let names = wildcard_excluded(options)
            .into_iter()
            .chain(wildcard_renamed(options).into_iter().map(|r| &r.ident))
            .chain(
                options
                    .opt_replace
                    .iter()
                    .flat_map(|r| r.items.iter().map(|e| &e.column_name)),
            );
        for name in names {
            match qualifier {
                Some(qualifier) => self.column(Some(qualifier), name),
                // Only a name no relation has: `EXCLUDE (id)` over a join is fine
                None => {
                    if matches!(
                        self.scope.column(&name.value, self.dialect),
                        ColumnLookup::NotFound
                    ) {
                        self.column(None, name);
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // FROM
    // ---------------------------------------------------------------------

    /// Resolve a FROM item with its joins, registering its relations in the current frame
    fn table_with_joins(&mut self, table: &TableWithJoins) {
        self.table_factor(&table.relation);
        self.forget_duplicate_relations();
        for join in &table.joins {
            self.table_factor(&join.relation);
            self.forget_duplicate_relations();
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
            JoinConstraint::On(expr) => {
                self.join_on(expr);
                self.check_aggregates_in(expr, Clause::JoinOn);
            }
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
                if let Some(table_args) = args {
                    let mut relation = None;
                    let mut columns = None;
                    if self.dialect == SqlDialect::Snowflake {
                        relation = self.identifier_relation(name, &table_args.args);
                        if relation.is_none() {
                            columns = self.snowflake_table_function(factor);
                        }
                    }
                    // Without an alias, the table (`IDENTIFIER('t')`) or else the
                    // function name names the relation
                    let table_key = relation
                        .as_ref()
                        .and_then(|r| r.name.rsplit('.').next().map(str::to_string));
                    if let Some(key) = alias_name
                        .or(table_key)
                        .or_else(|| name.0.last().map(|i| i.value.clone()))
                    {
                        let mut relation = relation.unwrap_or_else(|| {
                            Relation::new(RelationKind::Function, key.clone(), columns)
                        });
                        relation.columns =
                            Relation::rename_columns(relation.columns, &alias_columns);
                        self.scope.current().relations.insert(key, relation);
                    }
                    return;
                }

                let aliased = alias_name.is_some();
                let mut key = alias_name.unwrap_or_else(|| table_name.name.clone());
                let cte = table_name
                    .schema
                    .is_none()
                    .then(|| self.scope.cte(self.catalog, &table_name.name))
                    .flatten();
                // BigQuery: a wildcard table, or a table of a dataset the schema
                // doesn't define
                let may_exist = self.dialect == SqlDialect::BigQuery
                    && self.bigquery_table_may_exist(&table_name);
                // BigQuery: a table of another project than the ones the schema
                // describes is unknown, even if a dataset of the schema has a table
                // with its name
                let other_project =
                    self.dialect == SqlDialect::BigQuery && !self.catalog.database_is_known(name);
                let mut relation = if other_project {
                    Relation::new(RelationKind::Opaque, table_name.to_string(), None)
                } else if let Some(cte) = cte {
                    Relation::new(RelationKind::Cte, cte.name.clone(), cte.columns.clone())
                } else if let Some(table) = Relation::table(self.catalog, &table_name) {
                    table
                } else if let Some(view) = Relation::view(self.catalog, &table_name) {
                    view
                } else {
                    if !is_system_table(self.dialect, &table_name) && !may_exist {
                        let span = name.0.last().map(|id| Span::from_sqlparser(&id.span));
                        let diag = self.table_not_found(&table_name, span);
                        self.diagnostics.push(diag);
                    }
                    // Unknown columns, so references to a missing table don't cascade
                    // into column/alias errors
                    Relation::new(RelationKind::Opaque, table_name.to_string(), None)
                };
                if let Some(alias) = alias {
                    self.check_alias_count(
                        "Table",
                        &alias.name,
                        relation.columns.as_deref(),
                        alias_columns.len(),
                        false,
                    );
                }
                relation.columns = Relation::rename_columns(relation.columns, &alias_columns);
                if aliased {
                    // The alias hides the table name
                    relation.qualified = None;
                } else if let Some(qualified) = &relation.qualified {
                    // `FROM auth.users JOIN public.users`: both are `users`, told apart
                    // by their schema-qualified names
                    let other_schema = lookup_ignore_case(&self.scope.current().relations, &key)
                        .and_then(|r| r.qualified.as_ref())
                        .is_some_and(|q| q != qualified);
                    if other_schema {
                        key = qualified.to_string();
                    }
                }
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
                    self.check_alias_count(
                        "Subquery",
                        &a.name,
                        columns.as_deref(),
                        names.len(),
                        true,
                    );
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
                // UNNEST(t.items) sees the FROM items before it
                if let TableFactor::UNNEST { array_exprs, .. } = factor {
                    for e in array_exprs {
                        self.expr(e);
                    }
                }
                // Table-valued functions (e.g., generate_series, unnest): columns are
                // known only from an alias column list (or Snowflake's FLATTEN)
                let columns = if self.dialect == SqlDialect::Snowflake {
                    self.snowflake_table_function(factor)
                } else {
                    None
                };
                let (key, names) = match alias {
                    Some(a) => (
                        a.name.value.clone(),
                        a.columns.iter().map(|c| c.name.value.clone()).collect(),
                    ),
                    None => (format!("{factor}"), Vec::new()),
                };
                let columns = Relation::rename_columns(columns, &names);
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
            TableFactor::Unpivot {
                table,
                value,
                name,
                columns: unpivoted,
                alias,
            } => {
                let start = self.scope.current().relations.len();
                self.table_factor(table);
                for column in unpivoted {
                    self.column(None, column);
                }
                // The input's columns but the unpivoted ones, then the name and
                // value columns
                let (key, columns) = self.take_relations_since(start);
                let columns = columns.map(|mut columns| {
                    columns.retain(|c| {
                        !unpivoted
                            .iter()
                            .any(|u| u.value.eq_ignore_ascii_case(&c.name))
                    });
                    columns.push(Column::new(
                        name.value.clone(),
                        ExpressionType::Known(SqlType::Text),
                    ));
                    columns.push(Column::new(value.value.clone(), ExpressionType::Unknown));
                    columns
                });
                self.insert_pivot_relation(alias.as_ref(), key, columns);
            }
            TableFactor::Pivot {
                table,
                aggregate_functions,
                value_column,
                alias,
                ..
            } => {
                let start = self.scope.current().relations.len();
                self.table_factor(table);
                for aggregate in aggregate_functions {
                    self.expr(&aggregate.expr);
                }
                for column in value_column {
                    self.column(None, column);
                }
                // Output columns are named after the pivoted values: unknown
                let (key, _) = self.take_relations_since(start);
                self.insert_pivot_relation(alias.as_ref(), key, None);
            }
            _ => {}
        }
    }

    /// Remove the relations registered in the current frame since it had `start`
    /// of them: the key of the first, and all their `SELECT *` columns (`None` if
    /// unknown)
    fn take_relations_since(&mut self, start: usize) -> (Option<String>, Option<Vec<Column>>) {
        let inner = self.scope.current().relations.split_off(start);
        let key = inner.keys().next().cloned();
        let columns = inner
            .values()
            .map(|r| {
                r.star_columns()
                    .map(|cols| cols.cloned().collect::<Vec<_>>())
            })
            .collect::<Option<Vec<_>>>()
            .map(|cols| cols.concat());
        (key, columns)
    }

    /// Register the result of PIVOT / UNPIVOT under its alias, or else under the
    /// name of its input
    fn insert_pivot_relation(
        &mut self,
        alias: Option<&TableAlias>,
        input_key: Option<String>,
        columns: Option<Vec<Column>>,
    ) {
        let Some(key) = alias.map(|a| a.name.value.clone()).or(input_key) else {
            return;
        };
        let names: Vec<String> = alias
            .iter()
            .flat_map(|a| a.columns.iter().map(|c| c.name.value.clone()))
            .collect();
        let columns = Relation::rename_columns(columns, &names);
        self.scope.current().relations.insert(
            key.clone(),
            Relation::new(RelationKind::Subquery, key, columns),
        );
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
            SelectItem::QualifiedWildcard(name, options) => {
                // [[database.]schema.]table.*
                let parts = split_dotted_idents(&name.0);
                if let [.., table] = parts.as_ref() {
                    let schema = parts.len().checked_sub(2).map(|i| &parts[i]);
                    let in_scope = self.scope.current_ref().is_some_and(|f| {
                        f.qualified_relation(self.catalog, schema, table).is_some()
                    });
                    if in_scope {
                        self.wildcard_options(Some((schema, table)), options);
                    } else if self.dialect.struct_field_paths() && self.bigquery_struct_star(&parts)
                    {
                        // `struct_col.*` / `alias.struct_col.field.*`: the STRUCT's
                        // fields, which sqlsift doesn't model
                    } else {
                        self.diagnostics.push(relation_not_in_from(schema, table));
                    }
                }
            }
            SelectItem::Wildcard(options) => {
                self.wildcard_options(None, options);
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
            Expr::CompoundIdentifier(idents) if self.dialect.struct_field_paths() => {
                self.bigquery_path(idents);
            }
            Expr::CompoundIdentifier(idents) => {
                // [[database.]schema.]table.column
                let parts = split_dotted_idents(idents);
                if let Some((schema, table, column)) = split_column_ref(&parts) {
                    self.column(Some((schema, table)), column);
                }
            }
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
                match (&func.args, self.date_part_arg(func)) {
                    // Snowflake date part keyword (`DATEADD(day, 1, d)`): not a column
                    (FunctionArguments::List(list), Some(index)) => {
                        for (i, arg) in list.args.iter().enumerate() {
                            if i != index {
                                self.function_arg(arg);
                            }
                        }
                    }
                    (args, _) => self.function_args(args),
                }
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
            // `(expr).field` (BigQuery `f(x).field`, rewritten): fields aren't modeled
            Expr::CompositeAccess { expr, .. } => self.expr(expr),
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
    fn function_args(&mut self, args: &FunctionArguments) {
        if let FunctionArguments::List(arg_list) = args {
            for arg in &arg_list.args {
                self.function_arg(arg);
            }
        }
    }

    /// Resolve one function argument
    fn function_arg(&mut self, arg: &FunctionArg) {
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

    /// Position of a Snowflake or Redshift date part keyword argument
    /// (`DATEADD(day, ...)`, `DATE_TRUNC(month, ...)`, `LAST_DAY(d, week)`,
    /// `DATE_PART(h, ts)`), which the parser represents as a column reference
    fn date_part_arg(&self, func: &sqlparser::ast::Function) -> Option<usize> {
        let is_date_part: fn(&str) -> bool = match self.dialect {
            SqlDialect::Snowflake => is_snowflake_date_part,
            SqlDialect::Redshift => is_redshift_date_part,
            _ => return None,
        };
        let name = func.name.0.last()?.value.to_ascii_uppercase();
        let index = match (name.as_str(), self.dialect) {
            (
                "DATEADD" | "TIMEADD" | "TIMESTAMPADD" | "DATEDIFF" | "TIMEDIFF" | "TIMESTAMPDIFF"
                | "DATE_TRUNC" | "DATE_PART",
                _,
            )
            | ("DATEPART" | "PGDATE_PART", SqlDialect::Redshift) => 0,
            ("LAST_DAY", SqlDialect::Snowflake) => 1,
            ("TIME_SLICE", SqlDialect::Snowflake) => 2,
            _ => return None,
        };
        let FunctionArguments::List(list) = &func.args else {
            return None;
        };
        match list.args.get(index)? {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Identifier(ident)))
                if ident.quote_style.is_none() && is_date_part(&ident.value) =>
            {
                Some(index)
            }
            _ => None,
        }
    }

    /// Snowflake table function in FROM (`LATERAL FLATTEN(input => ...)`,
    /// `TABLE(GENERATOR(...))`, `TABLE(SPLIT_TO_TABLE(...))`, ...): resolve its
    /// arguments, which can reference earlier FROM items, and return its columns
    /// if they are known (FLATTEN's)
    fn snowflake_table_function(&mut self, factor: &TableFactor) -> Option<Vec<Column>> {
        let (name, args): (&ObjectName, &[FunctionArg]) = match factor {
            TableFactor::Table {
                name,
                args: Some(args),
                ..
            } => (name, &args.args),
            TableFactor::Function { name, args, .. } => (name, args),
            TableFactor::TableFunction {
                expr: Expr::Function(func),
                ..
            } => match &func.args {
                FunctionArguments::List(list) => (&func.name, &list.args),
                _ => return None,
            },
            TableFactor::TableFunction { expr, .. } => {
                self.expr(expr);
                return None;
            }
            _ => return None,
        };
        for arg in args {
            self.function_arg(arg);
        }
        let is_flatten = name
            .0
            .last()
            .is_some_and(|f| f.value.eq_ignore_ascii_case("FLATTEN"));
        is_flatten.then(flatten_columns)
    }

    /// `IDENTIFIER('name')` in FROM (Snowflake): the table or view it names, if it is
    /// in the catalog
    fn identifier_relation(&self, name: &ObjectName, args: &[FunctionArg]) -> Option<Relation> {
        let is_identifier = name.0.len() == 1 && name.0[0].value.eq_ignore_ascii_case("IDENTIFIER");
        let [FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(
            sqlparser::ast::Value::SingleQuotedString(literal),
        )))] = args
        else {
            return None;
        };
        if !is_identifier {
            return None;
        }
        let parts: Vec<Ident> = literal
            .split('.')
            .map(|part| {
                let part = part.trim();
                match part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                    Some(quoted) => Ident::with_quote('"', quoted),
                    None => Ident::new(part),
                }
            })
            .collect();
        let table_name = self.catalog.qualified_name(&ObjectName(parts));
        Relation::table(self.catalog, &table_name)
            .or_else(|| Relation::view(self.catalog, &table_name))
    }

    /// Resolve a column reference, qualified by `[schema.]table` or not
    pub(super) fn column(
        &mut self,
        qualifier: Option<(Option<&Ident>, &Ident)>,
        column_ident: &Ident,
    ) {
        let column_name = &column_ident.value;
        let column_span = Span::from_sqlparser(&column_ident.span);

        // Qualified column reference (table.column, schema.table.column)
        if let Some((schema_id, table_id)) = qualifier {
            let Some(relation) = self
                .scope
                .qualified_relation(self.catalog, schema_id, table_id)
            else {
                self.diagnostics
                    .push(relation_not_in_from(schema_id, table_id));
                return;
            };
            match relation.column(column_name, self.dialect) {
                ColumnMatch::No => {}
                ColumnMatch::Duplicate => {
                    self.diagnostics
                        .push(duplicate_column(relation, column_name, column_span));
                    return;
                }
                ColumnMatch::Yes(_) | ColumnMatch::Maybe => return,
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
            // BigQuery: in GROUP BY, HAVING, QUALIFY and ORDER BY a name that is an
            // output column alias is the alias, even when input columns have it
            // (PostgreSQL's GROUP BY prefers input columns, so it stays ambiguous)
            ColumnLookup::Ambiguous(_)
                if self.dialect == SqlDialect::BigQuery
                    && self.scope.current_ref().is_some_and(|f| {
                        f.select_aliases
                            .iter()
                            .any(|a| a.eq_ignore_ascii_case(column_name))
                    }) => {}
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
            ColumnLookup::Duplicate(key) => {
                if let Some(relation) = self.scope.relation(&key) {
                    let diag = duplicate_column(relation, column_name, column_span);
                    self.diagnostics.push(diag);
                }
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
                        || (self.dialect == SqlDialect::BigQuery
                            && super::bigquery::is_date_part(column_name))
                        || (self.dialect == SqlDialect::Redshift
                            && is_redshift_niladic_function(column_name))
                        || column_name.starts_with('@'));
                // A script variable (`DECLARE x ...` earlier in the file)
                let is_variable = self
                    .variables
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(column_name));
                if is_alias || is_relation || is_keyword || is_variable {
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
        if col.nullable
            || col.default.is_some()
            || col.identity.is_some()
            || col.auto_increment
            || col.generated
        {
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
            self.off_search_path_hint(table_name),
            self.skipped_definition_hint(table_name),
            self.similar_table(table_name),
        ) {
            (Some(hint), _, _, _) | (None, Some(hint), _, _) | (None, None, Some(hint), _) => hint,
            (None, None, None, Some(suggestion)) => format!("Did you mean '{suggestion}'?"),
            (None, None, None, None) => self.missing_table_hint(table_name),
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

    /// Why an unqualified table is missing when a schema off the search path has it
    fn off_search_path_hint(&self, table_name: &QualifiedName) -> Option<String> {
        if table_name.schema.is_some() {
            return None;
        }
        let qualified = self.catalog.relation_off_search_path(&table_name.name)?;
        let schema = qualified.schema.as_deref().unwrap_or_default();
        Some(format!(
            "Did you mean '{qualified}'? Schema '{schema}' is not on the search_path, so its tables must be schema-qualified"
        ))
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

/// E0001 for a `[schema.]table` qualifier (of a column or `table.*`) that names no
/// relation of the FROM clause
fn relation_not_in_from(schema: Option<&Ident>, table: &Ident) -> Diagnostic {
    let name = match schema {
        Some(schema) => format!("{}.{}", schema.value, table.value),
        None => table.value.clone(),
    };
    Diagnostic::error(
        DiagnosticKind::TableNotFound,
        format!("Table or alias '{name}' not found in FROM clause"),
    )
    .with_span(Span::from_sqlparser(&table.span))
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
fn projection_aliases(projection: &[SelectItem], dialect: SqlDialect) -> Vec<String> {
    projection
        .iter()
        .filter_map(|item| match item {
            SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
            // Column name also acts as implicit alias
            SelectItem::UnnamedExpr(Expr::Identifier(ident)) => Some(ident.value.clone()),
            // BigQuery: so does the last name of a path (`t.a` is `a`)
            SelectItem::UnnamedExpr(Expr::CompoundIdentifier(idents))
                if dialect == SqlDialect::BigQuery =>
            {
                idents.last().map(|i| i.value.clone())
            }
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
        SqlDialect::Redshift => {
            matches!(schema.as_deref(), Some("pg_catalog" | "information_schema"))
                || table.starts_with("pg_")
                || table.starts_with("stl_")
                || table.starts_with("stv_")
                || table.starts_with("svl_")
                || table.starts_with("svv_")
                || table.starts_with("sys_")
        }
        SqlDialect::Snowflake => {
            matches!(
                schema.as_deref(),
                Some("information_schema" | "account_usage")
            )
        }
        // `INFORMATION_SCHEMA` views and `__TABLES__` metadata of a dataset
        SqlDialect::BigQuery => {
            schema
                .as_deref()
                .is_some_and(|s| s.contains("information_schema"))
                || table.contains("information_schema")
                || table == "__tables__"
        }
        SqlDialect::Databricks => matches!(schema.as_deref(), Some("information_schema")),
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

/// Columns removed by `SELECT * EXCLUDE (...)` (Snowflake) / `EXCEPT (...)` (BigQuery)
fn wildcard_excluded(options: &WildcardAdditionalOptions) -> Vec<&Ident> {
    let mut names: Vec<&Ident> = match &options.opt_exclude {
        Some(ExcludeSelectItem::Single(ident)) => vec![ident],
        Some(ExcludeSelectItem::Multiple(idents)) => idents.iter().collect(),
        None => Vec::new(),
    };
    if let Some(except) = &options.opt_except {
        names.push(&except.first_element);
        names.extend(&except.additional_elements);
    }
    names
}

/// Columns renamed by `SELECT * RENAME (a AS b, ...)` (Snowflake)
fn wildcard_renamed(options: &WildcardAdditionalOptions) -> Vec<&sqlparser::ast::IdentWithAlias> {
    match &options.opt_rename {
        Some(RenameSelectItem::Single(rename)) => vec![rename],
        Some(RenameSelectItem::Multiple(renames)) => renames.iter().collect(),
        None => Vec::new(),
    }
}

/// Output columns of Snowflake's FLATTEN table function
fn flatten_columns() -> Vec<Column> {
    let number = || ExpressionType::Known(SqlType::BigInt);
    let text = || ExpressionType::Known(SqlType::Text);
    vec![
        Column::new("SEQ", number()),
        Column::new("KEY", text()),
        Column::new("PATH", text()),
        Column::new("INDEX", number()),
        Column::new("VALUE", ExpressionType::Unknown),
        Column::new("THIS", ExpressionType::Unknown),
    ]
}

/// Redshift functions called without parentheses that the parser represents as
/// identifiers (`SYSDATE`, `CURRENT_USER`)
fn is_redshift_niladic_function(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "SYSDATE" | "CURRENT_USER" | "CURRENT_USER_ID" | "SESSION_USER" | "USER"
    )
}

/// Redshift date and time parts and their abbreviations, as accepted by DATEADD,
/// DATEDIFF, DATE_PART, DATE_TRUNC, ...
fn is_redshift_date_part(name: &str) -> bool {
    is_snowflake_date_part(name)
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "millennium"
                | "millennia"
                | "mil"
                | "mils"
                | "century"
                | "centuries"
                | "c"
                | "cent"
                | "cents"
                | "decade"
                | "decades"
                | "dec"
                | "decs"
                | "y"
                | "weeks"
                | "minutes"
                | "msecs"
                | "msecond"
                | "mseconds"
                | "millisec"
                | "millisecs"
                | "millisecon"
                | "microsec"
                | "microsecs"
                | "microsecond"
                | "microseconds"
                | "usecs"
                | "usecond"
                | "useconds"
                | "timezone"
        )
}

/// Snowflake date and time parts and their abbreviations, as accepted by DATEADD,
/// DATEDIFF, DATE_TRUNC, DATE_PART, LAST_DAY, ...
fn is_snowflake_date_part(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "year"
            | "y"
            | "yy"
            | "yyy"
            | "yyyy"
            | "yr"
            | "years"
            | "yrs"
            | "month"
            | "mm"
            | "mon"
            | "mons"
            | "months"
            | "day"
            | "d"
            | "dd"
            | "days"
            | "dayofmonth"
            | "dayofweek"
            | "weekday"
            | "dow"
            | "dw"
            | "dayofweekiso"
            | "weekday_iso"
            | "dow_iso"
            | "dw_iso"
            | "dayofyear"
            | "yearday"
            | "doy"
            | "dy"
            | "week"
            | "w"
            | "wk"
            | "weekofyear"
            | "woy"
            | "wy"
            | "weekiso"
            | "week_iso"
            | "weekofyeariso"
            | "weekofyear_iso"
            | "yearofweek"
            | "yearofweekiso"
            | "quarter"
            | "q"
            | "qtr"
            | "qtrs"
            | "quarters"
            | "hour"
            | "h"
            | "hh"
            | "hr"
            | "hours"
            | "hrs"
            | "minute"
            | "m"
            | "mi"
            | "min"
            | "minutes"
            | "mins"
            | "second"
            | "s"
            | "sec"
            | "seconds"
            | "secs"
            | "millisecond"
            | "ms"
            | "msec"
            | "milliseconds"
            | "microsecond"
            | "us"
            | "usec"
            | "microseconds"
            | "nanosecond"
            | "ns"
            | "nsec"
            | "nanosec"
            | "nsecond"
            | "nanoseconds"
            | "nanosecs"
            | "nseconds"
            | "epoch_second"
            | "epoch"
            | "epoch_seconds"
            | "epoch_millisecond"
            | "epoch_milliseconds"
            | "epoch_microsecond"
            | "epoch_microseconds"
            | "epoch_nanosecond"
            | "epoch_nanoseconds"
            | "timezone_hour"
            | "tzh"
            | "timezone_minute"
            | "tzm"
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
        Expr::CompositeAccess { key, .. } => Some(key.value.clone()),
        Expr::Case { .. } => Some("case".to_string()),
        _ => None,
    }
}

/// E0006 for a reference to a column name that a CTE or subquery outputs more than
/// once (`SELECT *` over a join of two relations that both have it)
fn duplicate_column(relation: &Relation, column: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        DiagnosticKind::AmbiguousColumn,
        format!(
            "Column '{}' is ambiguous ({} '{}' has more than one column named '{}')",
            column,
            relation.kind_name(),
            relation.name,
            column
        ),
    )
    .with_span(span)
    .with_help(format!(
        "Select the columns of {} by name and alias the duplicates instead of using '*'",
        relation.name
    ))
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
