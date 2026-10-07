//! Name resolver - resolves table and column references

use indexmap::IndexMap;
use sqlparser::ast::{
    Assignment, AssignmentTarget, Delete, Expr, GroupByExpr, Ident, Insert, Query, Select,
    SelectItem, SetExpr, Statement, Subscript, TableFactor, TableWithJoins, Values,
};
use sqlparser::ast::{
    ConflictTarget, Distinct, NamedWindowDefinition, NamedWindowExpr, OnConflictAction, OnInsert,
};
use std::collections::{HashMap, HashSet};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{Catalog, QualifiedName, TableDef};

/// Resolved table reference in a query
#[derive(Debug, Clone)]
pub(super) struct TableRef {
    /// The actual table definition
    pub(super) table: QualifiedName,
    /// Alias used in the query (if any)
    ///
    /// Note: Currently unused but reserved for future error message improvements
    /// to show the user-specified alias in diagnostics instead of the table name.
    #[allow(dead_code)]
    pub(super) alias: Option<String>,
    /// If this is a VIEW reference, the column names from the VIEW definition
    pub(super) view_columns: Option<Vec<String>>,
    /// If this is a derived table (subquery in FROM), the inferred column names
    pub(super) derived_columns: Option<Vec<String>>,
}

/// CTE (Common Table Expression) definition
#[derive(Debug, Clone)]
pub(super) struct CteDefinition {
    /// CTE name
    ///
    /// Note: Currently unused but may be useful for future diagnostic messages
    /// to reference the CTE by its original name.
    #[allow(dead_code)]
    pub(super) name: String,
    /// Column names inferred from the CTE query
    pub(super) columns: Vec<String>,
}

/// Name resolver for SQL queries
pub struct NameResolver<'a> {
    catalog: &'a Catalog,
    /// Current scope's table references (alias/name -> TableRef)
    pub(super) tables: IndexMap<String, TableRef>,
    /// Outer scope's table references (for correlated subqueries)
    outer_tables: IndexMap<String, TableRef>,
    /// CTEs available in current scope (name -> CteDefinition)
    pub(super) ctes: HashMap<String, CteDefinition>,
    /// SELECT aliases visible in ORDER BY (set before resolving ORDER BY)
    select_aliases: Vec<String>,
    /// Columns merged by `JOIN ... USING` / `NATURAL JOIN` in the current query
    /// (lowercase); unqualified references to them are not ambiguous
    using_columns: HashSet<String>,
    /// Collected diagnostics
    diagnostics: Vec<Diagnostic>,
    /// SQL dialect (system columns/tables, dialect keywords)
    dialect: SqlDialect,
}

impl<'a> NameResolver<'a> {
    /// Create a new name resolver for the given catalog
    ///
    /// The resolver will use the catalog to validate table and column references.
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            tables: IndexMap::new(),
            outer_tables: IndexMap::new(),
            select_aliases: Vec::new(),
            using_columns: HashSet::new(),
            ctes: HashMap::new(),
            diagnostics: Vec::new(),
            dialect: SqlDialect::default(),
        }
    }

    /// Set the SQL dialect
    pub fn with_dialect(mut self, dialect: SqlDialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// Resolve names in a statement
    ///
    /// Validates all table and column references in the statement against the catalog.
    /// Diagnostics are collected internally and can be retrieved with `into_diagnostics()`.
    pub fn resolve_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Query(query) => self.resolve_query(query),
            Statement::Insert(insert) => {
                self.resolve_insert(insert);
            }
            Statement::Update {
                table,
                assignments,
                from,
                selection,
                returning,
                ..
            } => {
                self.resolve_update(table, assignments, from.as_ref(), selection.as_ref());
                self.resolve_returning(returning.as_deref());
            }
            Statement::Delete(delete) => {
                self.resolve_delete(delete);
            }
            _ => {}
        }
    }

    /// Resolve names in an INSERT statement
    fn resolve_insert(&mut self, insert: &Insert) {
        let table_name = self.catalog.qualified_name(&insert.table_name);

        // Check if table exists
        let table_def = if let Some(def) = self.catalog.get_table(&table_name) {
            def
        } else if let Some(view) = self.catalog.get_view(&table_name) {
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
                self.resolve_set_expr(&source.body);
            }
            return;
        } else {
            let table_span = insert
                .table_name
                .0
                .last()
                .map(|id| Span::from_sqlparser(&id.span));
            let diag = self.table_not_found(&table_name, table_span);
            self.diagnostics.push(diag);
            return;
        };

        // Check if specified columns exist
        let specified_columns: Vec<&Ident> = insert.columns.iter().collect();
        for col_ident in &specified_columns {
            if !table_def.column_exists(&col_ident.value) {
                let similar = find_similar_column(table_def, &col_ident.value);
                let mut diag = Diagnostic::error(
                    DiagnosticKind::ColumnNotFound,
                    format!(
                        "Column '{}' not found in table '{}'",
                        col_ident.value, table_name
                    ),
                )
                .with_span(Span::from_sqlparser(&col_ident.span));
                if let Some(suggestion) = similar {
                    diag = diag.with_help(format!("Did you mean '{}'?", suggestion));
                }
                self.diagnostics.push(diag);
            }
        }

        // Check column count vs value count
        // (VALUES literals carry no source location, so point at the table name)
        let insert_span = insert
            .table_name
            .0
            .last()
            .map(|id| Span::from_sqlparser(&id.span));
        if let Some(source) = &insert.source {
            if let SetExpr::Values(Values { rows, .. }) = source.body.as_ref() {
                let expected_count = if specified_columns.is_empty() {
                    table_def.columns.len()
                } else {
                    specified_columns.len()
                };

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
                            .with_help(if specified_columns.is_empty() {
                                format!(
                                    "Table '{}' has {} columns. Specify columns explicitly or provide {} values",
                                    table_name, expected_count, expected_count
                                )
                            } else {
                                format!("Provide {} value(s) to match the column list", expected_count)
                            });
                        diag.span = insert_span;
                        self.diagnostics.push(diag);
                    }

                    // Resolve expressions in values (for subqueries, etc.)
                    for expr in row {
                        self.resolve_expr(expr);
                    }
                }
            } else {
                // INSERT ... SELECT - resolve the subquery in its own scope
                let saved_tables = std::mem::take(&mut self.tables);
                self.resolve_query(source);
                self.tables = saved_tables;

                let expected_count = if specified_columns.is_empty() {
                    table_def.columns.len()
                } else {
                    specified_columns.len()
                };
                let selected = self.infer_cte_columns(&source.body);
                if !selected.is_empty() && selected.len() != expected_count {
                    let mut diag = Diagnostic::error(
                        DiagnosticKind::ColumnCountMismatch,
                        format!(
                            "INSERT ... SELECT returns {} column(s) but {} column(s) were specified",
                            selected.len(),
                            expected_count
                        ),
                    )
                    .with_help(format!(
                        "Select {} column(s) to match the column list",
                        expected_count
                    ));
                    diag.span = insert_span;
                    self.diagnostics.push(diag);
                }
            }
        }

        // ON CONFLICT / ON DUPLICATE KEY UPDATE and RETURNING see the target table
        let key = insert
            .table_alias
            .as_ref()
            .map_or_else(|| table_name.name.clone(), |a| a.value.clone());
        let target = TableRef {
            table: table_name.clone(),
            alias: insert.table_alias.as_ref().map(|a| a.value.clone()),
            view_columns: None,
            derived_columns: None,
        };
        self.tables.insert(key, target.clone());
        if let Some(on) = &insert.on {
            // MySQL row alias: INSERT ... VALUES (...) AS new ON DUPLICATE KEY UPDATE c = new.c
            if let Some(row_alias) = insert
                .insert_alias
                .as_ref()
                .and_then(|a| a.row_alias.0.last())
            {
                self.tables.insert(row_alias.value.clone(), target.clone());
            }
            self.resolve_on_insert(on, table_def, &table_name, target);
            // EXCLUDED / the row alias are only visible in the ON clause
            self.tables.shift_remove("excluded");
            if let Some(row_alias) = insert
                .insert_alias
                .as_ref()
                .and_then(|a| a.row_alias.0.last())
            {
                self.tables.shift_remove(&row_alias.value);
            }
        }
        self.resolve_returning(insert.returning.as_deref());
    }

    /// Resolve `ON CONFLICT ...` / `ON DUPLICATE KEY UPDATE ...` of an INSERT
    fn resolve_on_insert(
        &mut self,
        on: &OnInsert,
        table_def: &TableDef,
        table_name: &QualifiedName,
        target: TableRef,
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
                        self.tables.insert("excluded".to_string(), target);
                        if let Some(selection) = &update.selection {
                            self.resolve_expr(selection);
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
            self.resolve_expr(&assignment.value);
        }
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
        if let Some(suggestion) = find_similar_column(table_def, &col.value) {
            diag = diag.with_help(format!("Did you mean '{}'?", suggestion));
        }
        self.diagnostics.push(diag);
    }

    /// Resolve a RETURNING list against the statement's tables
    fn resolve_returning(&mut self, returning: Option<&[SelectItem]>) {
        let Some(items) = returning else {
            return;
        };
        let span = Span::with_location(0, 0, 0);
        for item in items {
            self.resolve_select_item(item, &span);
        }
    }

    /// Resolve names in an UPDATE statement
    fn resolve_update(
        &mut self,
        table: &TableWithJoins,
        assignments: &[Assignment],
        from: Option<&TableWithJoins>,
        selection: Option<&Expr>,
    ) {
        // Resolve and register the table
        self.resolve_table_with_joins(table);

        // Resolve FROM clause (PostgreSQL: UPDATE ... FROM ...)
        if let Some(from_table) = from {
            self.resolve_table_with_joins(from_table);
        }

        // Get table definition for column validation
        let table_name = self.table_factor_name(&table.relation);
        let table_def = table_name.as_ref().and_then(|n| self.catalog.get_table(n));

        // Resolve SET clause columns
        for assignment in assignments {
            match &assignment.target {
                AssignmentTarget::ColumnName(col_name) if col_name.0.len() >= 2 => {
                    // `SET alias.col = ...` (MySQL multi-table UPDATE)
                    let n = col_name.0.len();
                    self.resolve_column(Some(&col_name.0[n - 2]), &col_name.0[n - 1]);
                }
                AssignmentTarget::ColumnName(col_name) if !table.joins.is_empty() => {
                    // Unqualified target in a multi-table UPDATE: any joined table
                    if let Some(col_ident) = col_name.0.last() {
                        self.resolve_column(None, col_ident);
                    }
                }
                AssignmentTarget::ColumnName(col_name) => {
                    // Get the column identifier
                    if let Some(col_ident) = col_name.0.last() {
                        if let Some(def) = table_def {
                            if !def.column_exists(&col_ident.value) {
                                let similar = find_similar_column(def, &col_ident.value);
                                let mut diag = Diagnostic::error(
                                    DiagnosticKind::ColumnNotFound,
                                    format!(
                                        "Column '{}' not found in table '{}'",
                                        col_ident.value,
                                        table_name
                                            .as_ref()
                                            .map(|n| n.to_string())
                                            .unwrap_or_default()
                                    ),
                                )
                                .with_span(Span::from_sqlparser(&col_ident.span));
                                if let Some(suggestion) = similar {
                                    diag =
                                        diag.with_help(format!("Did you mean '{}'?", suggestion));
                                }
                                self.diagnostics.push(diag);
                            }
                        }
                    }
                }
                AssignmentTarget::Tuple(_) => {
                    // Tuple assignment (col1, col2) = (val1, val2) - not commonly used
                }
            }

            // Resolve the value expression
            self.resolve_expr(&assignment.value);
        }

        // Resolve WHERE clause
        if let Some(where_expr) = selection {
            self.resolve_expr(where_expr);
        }
    }

    /// Resolve names in a DELETE statement
    fn resolve_delete(&mut self, delete: &Delete) {
        // Get the table from the FROM clause
        let tables = match &delete.from {
            sqlparser::ast::FromTable::WithFromKeyword(tables) => tables,
            sqlparser::ast::FromTable::WithoutKeyword(tables) => tables,
        };

        // Resolve USING clause first (PostgreSQL / MySQL: DELETE ... USING ...)
        if let Some(using_tables) = &delete.using {
            for table in using_tables {
                self.resolve_table_with_joins(table);
            }
        }

        // Resolve and register tables from FROM clause. With MySQL's
        // `DELETE FROM t1 USING t1 JOIN t2`, FROM names aliases of USING tables.
        for table in tables {
            let is_using_alias = delete.using.is_some()
                && table.joins.is_empty()
                && matches!(&table.relation, TableFactor::Table { name, alias: None, .. }
                    if name.0.len() == 1
                        && lookup_ignore_case(&self.tables, &name.0[0].value).is_some());
            if !is_using_alias {
                self.resolve_table_with_joins(table);
            }
        }

        // Resolve WHERE clause
        if let Some(where_expr) = &delete.selection {
            self.resolve_expr(where_expr);
        }

        self.resolve_returning(delete.returning.as_deref());
    }

    /// Resolve names in a query
    fn resolve_query(&mut self, query: &Query) {
        let saved_using = std::mem::take(&mut self.using_columns);
        self.resolve_query_inner(query);
        self.using_columns = saved_using;
    }

    fn resolve_query_inner(&mut self, query: &Query) {
        // Handle CTEs (WITH clause)
        if let Some(with) = &query.with {
            let is_recursive = with.recursive;

            for cte in &with.cte_tables {
                let cte_name = self.catalog.ident_name(&cte.alias.name);

                // For recursive CTEs, infer columns and register the CTE *before*
                // resolving the body, so the recursive part can reference itself.
                let columns = if !cte.alias.columns.is_empty() {
                    cte.alias
                        .columns
                        .iter()
                        .map(|c| c.name.value.clone())
                        .collect()
                } else {
                    self.infer_cte_columns(&cte.query.body)
                };

                if is_recursive {
                    // Pre-register the CTE so recursive references resolve
                    self.ctes.insert(
                        cte_name.clone(),
                        CteDefinition {
                            name: cte_name.clone(),
                            columns: columns.clone(),
                        },
                    );
                }

                // Save current table scope
                let saved_tables = self.tables.clone();

                // Resolve the CTE query (to validate it) in isolated scope
                self.resolve_set_expr(&cte.query.body);

                // Restore table scope (CTEs shouldn't pollute outer scope with their internal tables)
                self.tables = saved_tables;

                // Register the CTE (or update if already pre-registered)
                self.ctes.insert(
                    cte_name.clone(),
                    CteDefinition {
                        name: cte_name,
                        columns,
                    },
                );
            }
        }

        // Resolve the main query body
        self.resolve_set_expr(&query.body);

        // LIMIT / OFFSET expressions (e.g. scalar subqueries)
        if let Some(limit) = &query.limit {
            self.resolve_expr(limit);
        }
        if let Some(offset) = &query.offset {
            self.resolve_expr(&offset.value);
        }

        // Resolve ORDER BY clause (with SELECT aliases in scope)
        if let Some(order_by) = &query.order_by {
            // Collect SELECT aliases so ORDER BY can reference them
            let saved_aliases = std::mem::take(&mut self.select_aliases);
            self.select_aliases = match query.body.as_ref() {
                // ORDER BY of a set operation refers to its output columns
                SetExpr::SetOperation { .. } => self.infer_cte_columns(&query.body),
                body => self.collect_select_aliases(body),
            };
            for ob in &order_by.exprs {
                self.resolve_expr(&ob.expr);
            }
            self.select_aliases = saved_aliases;
        }
    }

    /// Collect aliases from SELECT projection for use in ORDER BY resolution
    fn collect_select_aliases(&self, set_expr: &SetExpr) -> Vec<String> {
        match set_expr {
            SetExpr::Select(select) => projection_aliases(&select.projection),
            _ => Vec::new(),
        }
    }

    /// Infer column names from a SELECT body
    ///
    /// Returns an empty list when the columns can't be determined (e.g. `SELECT *`
    /// over a table function), which callers treat as "any column".
    fn infer_cte_columns(&self, set_expr: &SetExpr) -> Vec<String> {
        // For UNION/INTERSECT/EXCEPT, infer from the left side
        if let SetExpr::SetOperation { left, .. } = set_expr {
            return self.infer_cte_columns(left);
        }

        let (select_items, from): (Option<&[SelectItem]>, &[TableWithJoins]) = match set_expr {
            SetExpr::Select(select) => (Some(&select.projection), &select.from),
            SetExpr::Query(query) => return self.infer_cte_columns(&query.body),
            SetExpr::Insert(Statement::Insert(Insert { returning, .. })) => {
                (returning.as_deref(), &[])
            }
            SetExpr::Update(Statement::Update { returning, .. }) => (returning.as_deref(), &[]),
            _ => (None, &[]),
        };

        let mut columns = Vec::new();
        if let Some(items) = select_items {
            for item in items {
                match item {
                    SelectItem::ExprWithAlias { alias, .. } => {
                        columns.push(alias.value.clone());
                    }
                    SelectItem::UnnamedExpr(expr) => {
                        columns.push(
                            implicit_column_name(expr).unwrap_or_else(|| "?column?".to_string()),
                        );
                    }
                    SelectItem::Wildcard(_) => {
                        // Expand * from every relation in FROM
                        let relations = self.columns_of_from_items(from);
                        let Some(relations) = relations else {
                            return Vec::new();
                        };
                        columns.extend(relations.into_iter().flat_map(|(_, cols)| cols));
                    }
                    SelectItem::QualifiedWildcard(name, _) => {
                        // Expand t.* from the matching relation
                        let qualifier = name.0.last().map(|i| i.value.as_str()).unwrap_or("");
                        let relation = self.columns_of_from_items(from).and_then(|relations| {
                            relations
                                .into_iter()
                                .find(|(n, _)| n.eq_ignore_ascii_case(qualifier))
                        });
                        let Some((_, cols)) = relation else {
                            return Vec::new();
                        };
                        columns.extend(cols);
                    }
                }
            }
        }

        columns
    }

    /// Column names of each relation in a FROM clause, keyed by alias (or table name).
    /// Returns `None` if any relation's columns can't be determined.
    fn columns_of_from_items(&self, from: &[TableWithJoins]) -> Option<Vec<(String, Vec<String>)>> {
        let mut relations = Vec::new();
        for table in from {
            for factor in
                std::iter::once(&table.relation).chain(table.joins.iter().map(|j| &j.relation))
            {
                relations.push(self.relation_columns(factor)?);
            }
        }
        Some(relations)
    }

    /// Column names of a single FROM relation, with its alias (or table name)
    fn relation_columns(&self, factor: &TableFactor) -> Option<(String, Vec<String>)> {
        match factor {
            TableFactor::Table {
                name,
                alias,
                args: None,
                ..
            } => {
                let table_name = self.catalog.qualified_name(name);
                let key = alias
                    .as_ref()
                    .map_or_else(|| table_name.name.clone(), |a| a.name.value.clone());
                let columns = if let Some(cte) = self.cte(&table_name.name) {
                    cte.columns.clone()
                } else if let Some(table_def) = self.catalog.get_table(&table_name) {
                    table_def.columns.keys().cloned().collect()
                } else {
                    self.catalog.get_view(&table_name)?.columns.clone()
                };
                (!columns.is_empty()).then_some((key, columns))
            }
            TableFactor::Derived {
                subquery,
                alias: Some(alias),
                ..
            } => {
                let columns = if alias.columns.is_empty() {
                    self.infer_cte_columns(&subquery.body)
                } else {
                    alias.columns.iter().map(|c| c.name.value.clone()).collect()
                };
                (!columns.is_empty()).then(|| (alias.name.value.clone(), columns))
            }
            _ => None,
        }
    }

    /// Resolve names in a set expression (SELECT, UNION, etc.)
    fn resolve_set_expr(&mut self, set_expr: &SetExpr) {
        match set_expr {
            SetExpr::Select(select) => self.resolve_select(select),
            SetExpr::Query(query) => self.resolve_query(query),
            SetExpr::SetOperation { left, right, .. } => {
                // Each branch has its own FROM scope
                let saved_tables = self.tables.clone();
                self.resolve_set_expr(left);
                self.tables = saved_tables.clone();
                self.resolve_set_expr(right);
                self.tables = saved_tables;
            }
            SetExpr::Insert(stmt) => self.resolve_statement(stmt),
            SetExpr::Update(stmt) => self.resolve_statement(stmt),
            _ => {}
        }
    }

    /// Resolve names in a SELECT statement
    fn resolve_select(&mut self, select: &Select) {
        // First, resolve FROM clause to build table scope
        for table_with_joins in &select.from {
            self.resolve_table_with_joins(table_with_joins);
        }

        // DISTINCT ON (...) expressions
        if let Some(Distinct::On(exprs)) = &select.distinct {
            for expr in exprs {
                self.resolve_expr(expr);
            }
        }

        // Named windows: WINDOW w AS (PARTITION BY ... ORDER BY ...)
        for NamedWindowDefinition(_, window) in &select.named_window {
            if let NamedWindowExpr::WindowSpec(spec) = window {
                for e in &spec.partition_by {
                    self.resolve_expr(e);
                }
                for ob in &spec.order_by {
                    self.resolve_expr(&ob.expr);
                }
            }
        }

        // Then resolve SELECT items
        let select_span = Span::from_sqlparser(&select.select_token.0.span);
        for item in &select.projection {
            self.resolve_select_item(item, &select_span);
        }

        // Resolve WHERE clause
        if let Some(selection) = &select.selection {
            self.resolve_expr(selection);
        }

        // Resolve GROUP BY (output column aliases are allowed, like in ORDER BY)
        if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
            let saved_aliases = std::mem::replace(
                &mut self.select_aliases,
                projection_aliases(&select.projection),
            );
            for expr in exprs {
                self.resolve_expr(expr);
            }
            self.select_aliases = saved_aliases;
        }

        // Resolve HAVING
        if let Some(having) = &select.having {
            self.resolve_expr(having);
        }
    }

    /// Resolve a table reference in FROM clause
    fn resolve_table_with_joins(&mut self, table: &TableWithJoins) {
        self.resolve_table_factor(&table.relation);

        for join in &table.joins {
            self.resolve_table_factor(&join.relation);
            // Resolve join condition
            self.resolve_join_condition(&join.join_operator, &join.relation);
        }
    }

    /// Resolve JOIN condition (ON clause)
    fn resolve_join_condition(
        &mut self,
        join_op: &sqlparser::ast::JoinOperator,
        relation: &TableFactor,
    ) {
        use sqlparser::ast::JoinConstraint;
        use sqlparser::ast::JoinOperator::*;

        let constraint = match join_op {
            Inner(c) | LeftOuter(c) | RightOuter(c) | FullOuter(c) | LeftSemi(c) | RightSemi(c)
            | LeftAnti(c) | RightAnti(c) => Some(c),
            CrossJoin | CrossApply | OuterApply | AsOf { .. } | Anti(_) | Semi(_) => None,
        };

        if let Some(constraint) = constraint {
            match constraint {
                JoinConstraint::On(expr) => {
                    self.resolve_expr(expr);
                }
                JoinConstraint::Using(columns) => {
                    // USING columns exist in both sides by definition and are merged
                    // into a single unqualified column, so only check they exist
                    let right_key = relation_key(relation);
                    for col in columns {
                        let has = |resolver: &Self, t: &TableRef| {
                            resolver.has_unknown_columns(t)
                                || resolver.table_ref_has_column(t, &col.value)
                        };
                        let (right, left): (Vec<_>, Vec<_>) = self
                            .tables
                            .iter()
                            .partition(|(k, _)| right_key.as_deref() == Some(k.as_str()));
                        let in_right = right.is_empty() || right.iter().any(|(_, t)| has(self, t));
                        let in_left = left.iter().any(|(_, t)| has(self, t));
                        if !(in_right && in_left) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticKind::ColumnNotFound,
                                    format!("Column '{}' not found", col.value),
                                )
                                .with_span(Span::from_sqlparser(&col.span)),
                            );
                        }
                        self.using_columns.insert(col.value.to_lowercase());
                    }
                }
                JoinConstraint::Natural => {
                    // NATURAL JOIN merges every column the two sides have in common
                    if let Some(table_def) = self
                        .table_factor_name(relation)
                        .and_then(|n| self.catalog.get_table(&n))
                    {
                        self.using_columns
                            .extend(table_def.columns.keys().map(|c| c.to_lowercase()));
                    }
                }
                JoinConstraint::None => {}
            }
        }
    }

    /// Resolve a table factor (table name, subquery, etc.)
    fn resolve_table_factor(&mut self, factor: &TableFactor) {
        match factor {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                let table_name = self.catalog.qualified_name(name);

                // Table-valued function call (e.g., generate_series(...))
                // Register alias if present, skip table existence check
                if args.is_some() {
                    // Without an alias, the function name names the relation
                    let alias_name = alias
                        .as_ref()
                        .map(|a| a.name.value.clone())
                        .or_else(|| name.0.last().map(|i| i.value.clone()));
                    if let Some(a_name) = alias_name {
                        let columns = alias
                            .as_ref()
                            .map(|a| a.columns.iter().map(|c| c.name.value.clone()).collect())
                            .filter(|cols: &Vec<String>| !cols.is_empty())
                            .unwrap_or_default();
                        self.tables.insert(
                            a_name.clone(),
                            TableRef {
                                table: QualifiedName::new(&a_name),
                                alias: Some(a_name),
                                view_columns: None,
                                derived_columns: Some(columns),
                            },
                        );
                    }
                    return;
                }

                // Check if it's a CTE first
                let is_cte = self.cte(&table_name.name).is_some();

                // Check if table or view exists (in catalog or as CTE)
                let is_view = !is_cte && self.catalog.view_exists(&table_name);
                if !is_cte
                    && !is_view
                    && !self.catalog.table_exists(&table_name)
                    && !is_system_table(self.dialect, &table_name)
                {
                    // Get span from the last identifier (table name)
                    let table_span = name.0.last().map(|id| Span::from_sqlparser(&id.span));
                    let diag = self.table_not_found(&table_name, table_span);
                    self.diagnostics.push(diag);

                    // Register a placeholder with unknown columns, so references to the
                    // missing table don't cascade into column/alias errors
                    let alias_name = alias.as_ref().map(|a| a.name.value.clone());
                    let lookup_name = alias_name
                        .clone()
                        .unwrap_or_else(|| table_name.name.clone());
                    self.tables.insert(
                        lookup_name,
                        TableRef {
                            table: table_name,
                            alias: alias_name,
                            view_columns: None,
                            derived_columns: Some(Vec::new()),
                        },
                    );
                    return;
                }

                // System catalogs: columns unknown
                if is_system_table(self.dialect, &table_name)
                    && !self.catalog.table_exists(&table_name)
                {
                    let alias_name = alias.as_ref().map(|a| a.name.value.clone());
                    let lookup_name = alias_name
                        .clone()
                        .unwrap_or_else(|| table_name.name.clone());
                    self.tables.insert(
                        lookup_name,
                        TableRef {
                            table: table_name,
                            alias: alias_name,
                            view_columns: None,
                            derived_columns: Some(Vec::new()),
                        },
                    );
                    return;
                }

                // Get view columns if this is a view reference
                let view_columns = if is_view {
                    self.catalog
                        .get_view(&table_name)
                        .map(|v| v.columns.clone())
                } else {
                    None
                };

                // Register table in scope
                let alias_name = alias.as_ref().map(|a| a.name.value.clone());
                let lookup_name = alias_name
                    .clone()
                    .unwrap_or_else(|| table_name.name.clone());

                self.tables.insert(
                    lookup_name,
                    TableRef {
                        table: table_name,
                        alias: alias_name,
                        view_columns,
                        derived_columns: None,
                    },
                );
            }
            TableFactor::Derived {
                lateral,
                subquery,
                alias,
            } => {
                // Save current table scope so subquery resolution doesn't leak
                let saved_tables = self.tables.clone();
                let saved_outer = self.outer_tables.clone();

                // Non-LATERAL subqueries cannot reference outer FROM tables.
                // LATERAL subqueries can, with lower precedence than their own tables.
                if *lateral {
                    self.outer_tables.extend(self.tables.drain(..));
                } else {
                    self.tables.clear();
                }

                // Resolve subquery
                self.resolve_query(subquery);
                self.outer_tables = saved_outer;

                // Infer column names from the subquery projection
                let derived_columns = self.infer_cte_columns(&subquery.body);

                // Restore table scope
                self.tables = saved_tables;

                // Register derived table alias in outer scope
                if let Some(a) = alias {
                    let alias_name = a.name.value.clone();
                    // Use explicit column aliases if provided: (SELECT ...) AS v(col1, col2)
                    let columns = if !a.columns.is_empty() {
                        a.columns.iter().map(|c| c.name.value.clone()).collect()
                    } else {
                        derived_columns
                    };
                    self.tables.insert(
                        alias_name.clone(),
                        TableRef {
                            table: QualifiedName::new(&alias_name),
                            alias: Some(alias_name),
                            view_columns: None,
                            derived_columns: Some(columns),
                        },
                    );
                }
            }
            TableFactor::TableFunction { alias, .. }
            | TableFactor::Function { alias, .. }
            | TableFactor::UNNEST { alias, .. } => {
                // Table-valued functions (e.g., generate_series, unnest)
                // Register with the alias's column list, or with unknown columns
                let (alias_name, columns) = match alias {
                    Some(a) => (
                        a.name.value.clone(),
                        a.columns.iter().map(|c| c.name.value.clone()).collect(),
                    ),
                    None => (format!("{factor}"), Vec::new()),
                };
                {
                    self.tables.insert(
                        alias_name.clone(),
                        TableRef {
                            table: QualifiedName::new(&alias_name),
                            alias: Some(alias_name),
                            view_columns: None,
                            derived_columns: Some(columns),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    /// Resolve a SELECT item
    fn resolve_select_item(&mut self, item: &SelectItem, select_span: &Span) {
        match item {
            SelectItem::UnnamedExpr(expr) => self.resolve_expr(expr),
            SelectItem::ExprWithAlias { expr, .. } => self.resolve_expr(expr),
            SelectItem::QualifiedWildcard(name, _) => {
                // table.*
                if let Some(first_ident) = name.0.first() {
                    let table_name = &first_ident.value;
                    if lookup_ignore_case(&self.tables, table_name).is_none() {
                        let table_span = Span::from_sqlparser(&first_ident.span);
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::TableNotFound,
                                format!("Table or alias '{}' not found in FROM clause", table_name),
                            )
                            .with_span(table_span),
                        );
                    }
                }
            }
            SelectItem::Wildcard(_) => {
                // * - valid if we have at least one table
                if self.tables.is_empty() {
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

    /// Resolve an expression
    fn resolve_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Identifier(ident) => {
                // Simple column name - must exist in one of the tables
                self.resolve_column(None, ident);
            }
            Expr::CompoundIdentifier(idents) => {
                // table.column or schema.table.column
                match idents.as_slice() {
                    [table, column] => {
                        self.resolve_column(Some(table), column);
                    }
                    [_schema, table, column] => {
                        self.resolve_column(Some(table), column);
                    }
                    _ => {}
                }
            }
            Expr::BinaryOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::UnaryOp { expr, .. } => {
                self.resolve_expr(expr);
            }
            Expr::Nested(inner) => {
                self.resolve_expr(inner);
            }
            Expr::Function(func) => {
                self.resolve_function_args_list(&func.args);
                // ORDER BY inside aggregate arguments: array_agg(x ORDER BY y)
                if let sqlparser::ast::FunctionArguments::List(list) = &func.args {
                    for clause in &list.clauses {
                        if let sqlparser::ast::FunctionArgumentClause::OrderBy(order_by) = clause {
                            for ob in order_by {
                                self.resolve_expr(&ob.expr);
                            }
                        }
                    }
                }
                // WITHIN GROUP (ORDER BY ...)
                for ob in &func.within_group {
                    self.resolve_expr(&ob.expr);
                }
                // Resolve FILTER (WHERE ...) clause
                if let Some(filter) = &func.filter {
                    self.resolve_expr(filter);
                }
                // Resolve OVER (PARTITION BY ... ORDER BY ...) clause
                if let Some(sqlparser::ast::WindowType::WindowSpec(spec)) = &func.over {
                    for e in &spec.partition_by {
                        self.resolve_expr(e);
                    }
                    for ob in &spec.order_by {
                        self.resolve_expr(&ob.expr);
                    }
                }
            }
            Expr::InList { expr, list, .. } => {
                self.resolve_expr(expr);
                for e in list {
                    self.resolve_expr(e);
                }
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.resolve_expr(expr);
                let saved_tables = self.tables.clone();
                let saved_outer = self.outer_tables.clone();
                self.outer_tables.extend(self.tables.drain(..));
                self.resolve_query(subquery);
                self.tables = saved_tables;
                self.outer_tables = saved_outer;
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.resolve_expr(expr);
                self.resolve_expr(low);
                self.resolve_expr(high);
            }
            Expr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                if let Some(op) = operand {
                    self.resolve_expr(op);
                }
                for cond in conditions {
                    self.resolve_expr(cond);
                }
                for result in results {
                    self.resolve_expr(result);
                }
                if let Some(else_r) = else_result {
                    self.resolve_expr(else_r);
                }
            }
            Expr::Subquery(query) => {
                let saved_tables = self.tables.clone();
                let saved_outer = self.outer_tables.clone();
                self.outer_tables.extend(self.tables.drain(..));
                self.resolve_query(query);
                self.tables = saved_tables;
                self.outer_tables = saved_outer;
            }
            Expr::IsNull(e) | Expr::IsNotNull(e) => {
                self.resolve_expr(e);
            }
            Expr::Cast { expr, .. } => {
                self.resolve_expr(expr);
            }
            Expr::Extract { expr, .. } => {
                self.resolve_expr(expr);
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => {
                self.resolve_expr(expr);
                if let Some(from) = substring_from {
                    self.resolve_expr(from);
                }
                if let Some(for_expr) = substring_for {
                    self.resolve_expr(for_expr);
                }
            }
            Expr::Trim {
                expr, trim_what, ..
            } => {
                self.resolve_expr(expr);
                if let Some(what) = trim_what {
                    self.resolve_expr(what);
                }
            }
            Expr::Position { expr, r#in } => {
                self.resolve_expr(expr);
                self.resolve_expr(r#in);
            }
            Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
                self.resolve_expr(expr);
                self.resolve_expr(pattern);
            }
            Expr::IsTrue(e) | Expr::IsFalse(e) | Expr::IsNotTrue(e) | Expr::IsNotFalse(e) => {
                self.resolve_expr(e);
            }
            Expr::JsonAccess { value, .. } => {
                self.resolve_expr(value);
            }
            Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::Exists { subquery, .. } => {
                let saved_tables = self.tables.clone();
                let saved_outer = self.outer_tables.clone();
                self.outer_tables.extend(self.tables.drain(..));
                self.resolve_query(subquery);
                self.tables = saved_tables;
                self.outer_tables = saved_outer;
            }
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => {
                self.resolve_expr(timestamp);
                self.resolve_expr(time_zone);
            }
            Expr::Collate { expr, .. } => {
                self.resolve_expr(expr);
            }
            Expr::Ceil { expr, .. } | Expr::Floor { expr, .. } => {
                self.resolve_expr(expr);
            }
            Expr::Overlay {
                expr,
                overlay_what,
                overlay_from,
                overlay_for,
            } => {
                self.resolve_expr(expr);
                self.resolve_expr(overlay_what);
                self.resolve_expr(overlay_from);
                if let Some(for_expr) = overlay_for {
                    self.resolve_expr(for_expr);
                }
            }
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                self.resolve_expr(a);
                self.resolve_expr(b);
            }
            Expr::IsUnknown(e) | Expr::IsNotUnknown(e) => {
                self.resolve_expr(e);
            }
            Expr::SimilarTo { expr, pattern, .. } | Expr::RLike { expr, pattern, .. } => {
                self.resolve_expr(expr);
                self.resolve_expr(pattern);
            }
            Expr::Tuple(exprs) => {
                for e in exprs {
                    self.resolve_expr(e);
                }
            }
            Expr::Array(arr) => {
                for e in &arr.elem {
                    self.resolve_expr(e);
                }
            }
            Expr::Subscript { expr, subscript } => {
                self.resolve_expr(expr);
                match subscript.as_ref() {
                    Subscript::Index { index } => {
                        self.resolve_expr(index);
                    }
                    Subscript::Slice {
                        lower_bound,
                        upper_bound,
                        stride,
                    } => {
                        if let Some(lb) = lower_bound {
                            self.resolve_expr(lb);
                        }
                        if let Some(ub) = upper_bound {
                            self.resolve_expr(ub);
                        }
                        if let Some(s) = stride {
                            self.resolve_expr(s);
                        }
                    }
                }
            }
            Expr::Method(method) => {
                self.resolve_expr(&method.expr);
                for func in &method.method_chain {
                    self.resolve_function_args_list(&func.args);
                }
            }
            Expr::GroupingSets(sets) | Expr::Cube(sets) | Expr::Rollup(sets) => {
                for set in sets {
                    for e in set {
                        self.resolve_expr(e);
                    }
                }
            }
            // Literals, intervals, and other expressions don't need column resolution
            _ => {}
        }
    }

    /// Resolve function arguments (handles Named, ExprNamed, and Unnamed variants)
    fn resolve_function_args_list(&mut self, args: &sqlparser::ast::FunctionArguments) {
        if let sqlparser::ast::FunctionArguments::List(arg_list) = args {
            for arg in &arg_list.args {
                match arg {
                    sqlparser::ast::FunctionArg::Unnamed(
                        sqlparser::ast::FunctionArgExpr::Expr(e),
                    ) => {
                        self.resolve_expr(e);
                    }
                    sqlparser::ast::FunctionArg::Named { arg, .. }
                    | sqlparser::ast::FunctionArg::ExprNamed { arg, .. } => {
                        if let sqlparser::ast::FunctionArgExpr::Expr(e) = arg {
                            self.resolve_expr(e);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Check if a table reference contains the given column
    fn table_ref_has_column(&self, table_ref: &TableRef, column_name: &str) -> bool {
        if let Some(derived_cols) = &table_ref.derived_columns {
            derived_cols.is_empty()
                || derived_cols
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(column_name))
        } else if let Some(cte) = self.cte(&table_ref.table.name) {
            cte.columns
                .iter()
                .any(|c| c.eq_ignore_ascii_case(column_name))
        } else if let Some(view_cols) = &table_ref.view_columns {
            view_cols.is_empty()
                || view_cols
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(column_name))
        } else if let Some(table_def) = self.catalog.get_table(&table_ref.table) {
            table_def.column_exists(column_name) || is_system_column(self.dialect, column_name)
        } else {
            false
        }
    }

    /// Resolve a column reference
    fn resolve_column(&mut self, table_ident: Option<&Ident>, column_ident: &Ident) {
        let column_name = &column_ident.value;
        let column_span = Span::from_sqlparser(&column_ident.span);

        if let Some(table_id) = table_ident {
            let table_alias = &table_id.value;
            // Qualified column reference (table.column)
            if let Some(table_ref) = lookup_ignore_case(&self.tables, table_alias)
                .or_else(|| lookup_ignore_case(&self.outer_tables, table_alias))
            {
                // Check derived table first
                if let Some(derived_cols) = &table_ref.derived_columns {
                    // Empty column list means we can't validate (e.g., table-valued functions)
                    if !derived_cols.is_empty()
                        && !derived_cols
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(column_name))
                        && !derived_cols.iter().any(|c| c == "?column?")
                    {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::ColumnNotFound,
                                format!(
                                    "Column '{}' not found in subquery '{}'",
                                    column_name, table_alias
                                ),
                            )
                            .with_span(column_span),
                        );
                    }
                } else if let Some(cte) = self.cte(&table_ref.table.name) {
                    // Validate against CTE columns (unless they couldn't be inferred)
                    if !cte.columns.is_empty()
                        && !cte
                            .columns
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(column_name))
                        && !cte.columns.iter().any(|c| c == "?column?")
                    {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::ColumnNotFound,
                                format!(
                                    "Column '{}' not found in CTE '{}'",
                                    column_name, table_ref.table
                                ),
                            )
                            .with_span(column_span),
                        );
                    }
                } else if let Some(view_cols) = &table_ref.view_columns {
                    // Validate against VIEW columns (unless they couldn't be inferred)
                    if !view_cols.is_empty()
                        && !view_cols
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(column_name))
                    {
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::ColumnNotFound,
                                format!(
                                    "Column '{}' not found in view '{}'",
                                    column_name, table_ref.table
                                ),
                            )
                            .with_span(column_span),
                        );
                    }
                } else if let Some(table_def) = self.catalog.get_table(&table_ref.table) {
                    if !table_def.column_exists(column_name)
                        && !is_system_column(self.dialect, column_name)
                    {
                        let similar = find_similar_column(table_def, column_name);
                        let mut diag = Diagnostic::error(
                            DiagnosticKind::ColumnNotFound,
                            format!(
                                "Column '{}' not found in table '{}'",
                                column_name, table_ref.table
                            ),
                        )
                        .with_span(column_span);
                        if let Some(suggestion) = similar {
                            diag = diag.with_help(format!("Did you mean '{}'?", suggestion));
                        }
                        self.diagnostics.push(diag);
                    }
                }
            } else {
                let table_span = Span::from_sqlparser(&table_id.span);
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TableNotFound,
                        format!("Table or alias '{}' not found in FROM clause", table_alias),
                    )
                    .with_span(table_span),
                );
            }
        } else {
            // Unqualified column reference - search inner scope first, then outer
            let mut found_in: Vec<&str> = Vec::new();

            // Tables with unknown columns (missing tables, table functions without
            // a column list) could provide any column: they only matter when no
            // table with known columns does, and never make a column ambiguous
            for (name, table_ref) in &self.tables {
                if !self.has_unknown_columns(table_ref)
                    && self.table_ref_has_column(table_ref, column_name)
                {
                    found_in.push(name);
                }
            }

            // If not found in inner scope, check outer scope (correlated subqueries)
            if found_in.is_empty() {
                for (name, table_ref) in &self.outer_tables {
                    if !self.has_unknown_columns(table_ref)
                        && self.table_ref_has_column(table_ref, column_name)
                    {
                        found_in.push(name);
                    }
                }
            }

            if found_in.is_empty()
                && self
                    .tables
                    .values()
                    .chain(self.outer_tables.values())
                    .any(|t| self.has_unknown_columns(t))
            {
                return;
            }

            match found_in.len() {
                0 => {
                    // Check if it's a SELECT alias (valid in ORDER BY)
                    if self
                        .select_aliases
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(column_name))
                    {
                        return;
                    }

                    // Whole-row reference to a table (`json_agg(u)`, `row_to_json(t)`)
                    if lookup_ignore_case(&self.tables, column_name).is_some()
                        || lookup_ignore_case(&self.outer_tables, column_name).is_some()
                    {
                        return;
                    }

                    // Keywords and variables the parser represents as identifiers:
                    // DEFAULT, date/time units (`TIMESTAMPDIFF(DAY, ...)`), `@var`
                    if column_ident.quote_style.is_none()
                        && (column_name.eq_ignore_ascii_case("DEFAULT")
                            || is_date_part_keyword(column_name)
                            || column_name.starts_with('@'))
                    {
                        return;
                    }

                    // Column not found in any table
                    let mut suggestions = Vec::new();
                    for table_ref in self.tables.values() {
                        if let Some(table_def) = self.catalog.get_table(&table_ref.table) {
                            if let Some(s) = find_similar_column(table_def, column_name) {
                                suggestions.push(s);
                            }
                        }
                    }

                    let mut diag = Diagnostic::error(
                        DiagnosticKind::ColumnNotFound,
                        format!("Column '{}' not found", column_name),
                    )
                    .with_span(column_span);
                    if !suggestions.is_empty() {
                        diag = diag.with_help(format!("Did you mean '{}'?", suggestions[0]));
                    }
                    self.diagnostics.push(diag);
                }
                1 => {
                    // Found in exactly one table - OK
                }
                _ if self.using_columns.contains(&column_name.to_lowercase()) => {
                    // Merged by JOIN ... USING / NATURAL JOIN - not ambiguous
                }
                _ => {
                    // Ambiguous - found in multiple tables
                    self.diagnostics.push(
                        Diagnostic::error(
                            DiagnosticKind::AmbiguousColumn,
                            format!(
                                "Column '{}' is ambiguous (found in tables: {})",
                                column_name,
                                found_in.join(", ")
                            ),
                        )
                        .with_span(column_span)
                        .with_help(format!(
                            "Qualify the column with a table name: {}.{}",
                            found_in[0], column_name
                        )),
                    );
                }
            }
        }
    }

    /// Catalog name of a plain table reference in FROM
    fn table_factor_name(&self, factor: &TableFactor) -> Option<QualifiedName> {
        match factor {
            TableFactor::Table { name, .. } => Some(self.catalog.qualified_name(name)),
            _ => None,
        }
    }

    /// Look up a CTE by name, following the catalog's identifier case rules
    fn cte(&self, name: &str) -> Option<&CteDefinition> {
        self.ctes.get(name).or_else(|| {
            self.ctes
                .iter()
                .find(|(k, _)| self.catalog.names_match(k, name))
                .map(|(_, v)| v)
        })
    }

    /// Whether a table reference has an unknown column list (a missing table, a table
    /// function without column aliases, or a CTE/subquery whose columns can't be inferred),
    /// so any column may belong to it
    fn has_unknown_columns(&self, table_ref: &TableRef) -> bool {
        match (&table_ref.derived_columns, &table_ref.view_columns) {
            (Some(columns), _) | (None, Some(columns)) => columns.is_empty(),
            (None, None) => self
                .cte(&table_ref.table.name)
                .is_some_and(|cte| cte.columns.is_empty()),
        }
    }

    /// Build a "table not found" diagnostic, suggesting a similarly named table, view or CTE
    fn table_not_found(&self, table_name: &QualifiedName, span: Option<Span>) -> Diagnostic {
        let candidates = self
            .catalog
            .table_or_view_names()
            .into_iter()
            .map(|n| n.name)
            .chain(self.ctes.keys().cloned());
        let help = match find_similar_name(candidates, &table_name.name) {
            Some(suggestion) => format!("Did you mean '{}'?", suggestion),
            None => "Check that the table exists in your schema definition".to_string(),
        };
        let mut diag = Diagnostic::error(
            DiagnosticKind::TableNotFound,
            format!("Table '{}' not found", table_name),
        )
        .with_help(help);
        if let Some(span) = span {
            diag = diag.with_span(span);
        }
        diag
    }

    /// Consume the resolver and return collected diagnostics
    ///
    /// Returns all diagnostics collected during name resolution.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
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

/// Implicit system columns every table has
fn is_system_column(dialect: SqlDialect, column: &str) -> bool {
    let column = column.to_ascii_lowercase();
    match dialect {
        SqlDialect::PostgreSQL => matches!(
            column.as_str(),
            "ctid" | "xmin" | "xmax" | "cmin" | "cmax" | "tableoid"
        ),
        SqlDialect::SQLite => matches!(column.as_str(), "rowid" | "oid" | "_rowid_"),
        SqlDialect::MySQL => false,
    }
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
        _ => None,
    }
}

/// Find a similar column name (for suggestions)
fn find_similar_column(table: &TableDef, name: &str) -> Option<String> {
    find_similar_name(table.columns.keys().cloned(), name)
}

/// Find the candidate most similar to `name` (for "did you mean" suggestions)
pub(super) fn find_similar_name(
    candidates: impl IntoIterator<Item = String>,
    name: &str,
) -> Option<String> {
    let name_lower = name.to_lowercase();
    let mut best_match: Option<(usize, String)> = None;

    for candidate in candidates {
        let candidate_lower = candidate.to_lowercase();
        let distance = levenshtein_distance(&name_lower, &candidate_lower);
        // A name that is a prefix of the candidate (`author` -> `author_id`) is similar
        let is_prefix = name_lower.chars().count() >= 3 && candidate_lower.starts_with(&name_lower);

        // Allow roughly one edit per three characters (at least 1, at most 3)
        if (is_prefix || distance <= name_lower.chars().count().div_ceil(3).clamp(1, 3))
            && best_match
                .as_ref()
                .map_or(true, |(best, _)| distance < *best)
        {
            best_match = Some((distance, candidate));
        }
    }

    best_match.map(|(_, name)| name)
}

/// Look up a table reference by alias or name, falling back to a case-insensitive match
pub(super) fn lookup_ignore_case<'m, V>(map: &'m IndexMap<String, V>, key: &str) -> Option<&'m V> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// Simple Levenshtein distance implementation
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut dp = vec![vec![0; n + 1]; m + 1];

    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate() {
        *val = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[m][n]
}
