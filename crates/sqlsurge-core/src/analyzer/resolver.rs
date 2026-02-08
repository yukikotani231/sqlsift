//! Name resolver - resolves table and column references

use sqlparser::ast::{
    Assignment, AssignmentTarget, Delete, Expr, FunctionArguments, GroupByExpr, Ident, Insert,
    ObjectName, Query, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins, Values,
};
use std::collections::HashMap;

use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{Catalog, QualifiedName, TableDef};

/// Resolved table reference in a query
#[derive(Debug, Clone)]
struct TableRef {
    /// The actual table definition
    table: QualifiedName,
    /// Alias used in the query (if any)
    #[allow(dead_code)]
    alias: Option<String>,
    /// If this is a VIEW reference, the column names from the VIEW definition
    view_columns: Option<Vec<String>>,
    /// If this is a derived table (subquery in FROM), the inferred column names
    derived_columns: Option<Vec<String>>,
}

/// CTE (Common Table Expression) definition
#[derive(Debug, Clone)]
struct CteDefinition {
    /// CTE name
    #[allow(dead_code)]
    name: String,
    /// Column names inferred from the CTE query
    columns: Vec<String>,
}

/// Name resolver for SQL queries
pub struct NameResolver<'a> {
    catalog: &'a Catalog,
    /// Current scope's table references (alias/name -> TableRef)
    tables: HashMap<String, TableRef>,
    /// CTEs available in current scope (name -> CteDefinition)
    ctes: HashMap<String, CteDefinition>,
    /// Collected diagnostics
    diagnostics: Vec<Diagnostic>,
}

impl<'a> NameResolver<'a> {
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            tables: HashMap::new(),
            ctes: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Resolve names in a statement
    pub fn resolve_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Query(query) => self.resolve_query(query),
            Statement::Insert(insert) => {
                self.resolve_insert(insert);
            }
            Statement::Update {
                table,
                assignments,
                selection,
                ..
            } => {
                self.resolve_update(table, assignments, selection.as_ref());
            }
            Statement::Delete(delete) => {
                self.resolve_delete(delete);
            }
            _ => {}
        }
    }

    /// Resolve names in an INSERT statement
    fn resolve_insert(&mut self, insert: &Insert) {
        let table_name = object_name_to_qualified(&insert.table_name);

        // Check if table exists
        let table_def = if let Some(def) = self.catalog.get_table(&table_name) {
            def
        } else {
            let table_span = insert
                .table_name
                .0
                .last()
                .map(|id| Span::from_sqlparser(&id.span));
            let mut diag = Diagnostic::error(
                DiagnosticKind::TableNotFound,
                format!("Table '{}' not found", table_name),
            )
            .with_help("Check that the table exists in your schema definition");
            if let Some(span) = table_span {
                diag = diag.with_span(span);
            }
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
        if let Some(source) = &insert.source {
            if let SetExpr::Values(Values { rows, .. }) = source.body.as_ref() {
                let expected_count = if specified_columns.is_empty() {
                    table_def.columns.len()
                } else {
                    specified_columns.len()
                };

                for row in rows {
                    if row.len() != expected_count {
                        self.diagnostics.push(
                            Diagnostic::error(
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
                            }),
                        );
                    }

                    // Resolve expressions in values (for subqueries, etc.)
                    for expr in row {
                        self.resolve_expr(expr);
                    }
                }
            } else {
                // INSERT ... SELECT - resolve the subquery
                self.resolve_set_expr(&source.body);
            }
        }
    }

    /// Resolve names in an UPDATE statement
    fn resolve_update(
        &mut self,
        table: &TableWithJoins,
        assignments: &[Assignment],
        selection: Option<&Expr>,
    ) {
        // Resolve and register the table
        self.resolve_table_with_joins(table);

        // Get table definition for column validation
        let table_name = table_with_joins_to_name(&table.relation);
        let table_def = table_name.as_ref().and_then(|n| self.catalog.get_table(n));

        // Resolve SET clause columns
        for assignment in assignments {
            match &assignment.target {
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

        // Resolve and register tables from FROM clause
        for table in tables {
            self.resolve_table_with_joins(table);
        }

        // Resolve WHERE clause
        if let Some(where_expr) = &delete.selection {
            self.resolve_expr(where_expr);
        }
    }

    /// Resolve names in a query
    fn resolve_query(&mut self, query: &Query) {
        // Handle CTEs (WITH clause)
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                // Save current table scope
                let saved_tables = self.tables.clone();

                // Resolve the CTE query first (to validate it) in isolated scope
                self.resolve_set_expr(&cte.query.body);

                // Extract column names from the CTE
                let cte_name = cte.alias.name.value.clone();
                let columns = if !cte.alias.columns.is_empty() {
                    // Explicit column list: WITH foo (a, b) AS ...
                    cte.alias
                        .columns
                        .iter()
                        .map(|c| c.name.value.clone())
                        .collect()
                } else {
                    // Infer from SELECT projection
                    self.infer_cte_columns(&cte.query.body)
                };

                // Restore table scope (CTEs shouldn't pollute outer scope with their internal tables)
                self.tables = saved_tables;

                // Register the CTE
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
    }

    /// Infer column names from a SELECT body
    fn infer_cte_columns(&self, set_expr: &SetExpr) -> Vec<String> {
        let mut columns = Vec::new();

        if let SetExpr::Select(select) = set_expr {
            for (idx, item) in select.projection.iter().enumerate() {
                match item {
                    SelectItem::UnnamedExpr(Expr::Identifier(ident)) => {
                        columns.push(ident.value.clone());
                    }
                    SelectItem::ExprWithAlias { alias, .. } => {
                        columns.push(alias.value.clone());
                    }
                    SelectItem::UnnamedExpr(Expr::CompoundIdentifier(idents)) => {
                        // table.column -> use column name
                        if let Some(col) = idents.last() {
                            columns.push(col.value.clone());
                        }
                    }
                    SelectItem::Wildcard(_) => {
                        // Can't infer columns from * - would need to expand
                        // For now, skip validation of CTE columns when * is used
                    }
                    SelectItem::QualifiedWildcard(_, _) => {
                        // table.* - can't infer easily
                    }
                    _ => {
                        // Other expressions - generate a name
                        columns.push(format!("?column?{}", idx + 1));
                    }
                }
            }
        }

        columns
    }

    /// Resolve names in a set expression (SELECT, UNION, etc.)
    fn resolve_set_expr(&mut self, set_expr: &SetExpr) {
        match set_expr {
            SetExpr::Select(select) => self.resolve_select(select),
            SetExpr::Query(query) => self.resolve_query(query),
            SetExpr::SetOperation { left, right, .. } => {
                self.resolve_set_expr(left);
                self.resolve_set_expr(right);
            }
            _ => {}
        }
    }

    /// Resolve names in a SELECT statement
    fn resolve_select(&mut self, select: &Select) {
        // First, resolve FROM clause to build table scope
        for table_with_joins in &select.from {
            self.resolve_table_with_joins(table_with_joins);
        }

        // Then resolve SELECT items
        for item in &select.projection {
            self.resolve_select_item(item);
        }

        // Resolve WHERE clause
        if let Some(selection) = &select.selection {
            self.resolve_expr(selection);
        }

        // Resolve GROUP BY
        match &select.group_by {
            GroupByExpr::All(_) => {}
            GroupByExpr::Expressions(exprs, _) => {
                for expr in exprs {
                    self.resolve_expr(expr);
                }
            }
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
            self.resolve_join_condition(&join.join_operator);
        }
    }

    /// Resolve JOIN condition (ON clause)
    fn resolve_join_condition(&mut self, join_op: &sqlparser::ast::JoinOperator) {
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
                    // For USING clause, check that columns exist in both tables
                    for col in columns {
                        self.resolve_column(None, col);
                    }
                }
                JoinConstraint::Natural | JoinConstraint::None => {}
            }
        }
    }

    /// Resolve a table factor (table name, subquery, etc.)
    fn resolve_table_factor(&mut self, factor: &TableFactor) {
        match factor {
            TableFactor::Table { name, alias, .. } => {
                let table_name = object_name_to_qualified(name);

                // Check if it's a CTE first
                let is_cte = self.ctes.contains_key(&table_name.name);

                // Check if table or view exists (in catalog or as CTE)
                let is_view = !is_cte && self.catalog.view_exists(&table_name);
                if !is_cte && !is_view && !self.catalog.table_exists(&table_name) {
                    // Get span from the last identifier (table name)
                    let table_span = name.0.last().map(|id| Span::from_sqlparser(&id.span));
                    let mut diag = Diagnostic::error(
                        DiagnosticKind::TableNotFound,
                        format!("Table '{}' not found", table_name),
                    )
                    .with_help("Check that the table exists in your schema definition");
                    if let Some(span) = table_span {
                        diag = diag.with_span(span);
                    }
                    self.diagnostics.push(diag);
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

                // Non-LATERAL subqueries cannot reference outer FROM tables,
                // so clear the table scope. LATERAL subqueries can see outer tables.
                if !lateral {
                    self.tables.clear();
                }

                // Resolve subquery
                self.resolve_query(subquery);

                // Infer column names from the subquery projection
                let derived_columns = self.infer_cte_columns(&subquery.body);

                // Restore table scope
                self.tables = saved_tables;

                // Register derived table alias in outer scope
                if let Some(a) = alias {
                    let alias_name = a.name.value.clone();
                    self.tables.insert(
                        alias_name.clone(),
                        TableRef {
                            table: QualifiedName::new(&alias_name),
                            alias: Some(alias_name),
                            view_columns: None,
                            derived_columns: Some(derived_columns),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    /// Resolve a SELECT item
    fn resolve_select_item(&mut self, item: &SelectItem) {
        match item {
            SelectItem::UnnamedExpr(expr) => self.resolve_expr(expr),
            SelectItem::ExprWithAlias { expr, .. } => self.resolve_expr(expr),
            SelectItem::QualifiedWildcard(name, _) => {
                // table.*
                if let Some(first_ident) = name.0.first() {
                    let table_name = &first_ident.value;
                    if !self.tables.contains_key(table_name.as_str()) {
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
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticKind::TableNotFound,
                        "SELECT * requires at least one table in FROM clause",
                    ));
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
            Expr::Function(func) => match &func.args {
                FunctionArguments::None => {}
                FunctionArguments::Subquery(_) => {}
                FunctionArguments::List(args) => {
                    for arg in &args.args {
                        if let sqlparser::ast::FunctionArg::Unnamed(
                            sqlparser::ast::FunctionArgExpr::Expr(e),
                        ) = arg
                        {
                            self.resolve_expr(e);
                        }
                    }
                }
            },
            Expr::InList { expr, list, .. } => {
                self.resolve_expr(expr);
                for e in list {
                    self.resolve_expr(e);
                }
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.resolve_expr(expr);
                self.resolve_query(subquery);
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
                self.resolve_query(query);
            }
            Expr::IsNull(e) | Expr::IsNotNull(e) => {
                self.resolve_expr(e);
            }
            // Literals and other expressions don't need resolution
            _ => {}
        }
    }

    /// Resolve a column reference
    fn resolve_column(&mut self, table_ident: Option<&Ident>, column_ident: &Ident) {
        let column_name = &column_ident.value;
        let column_span = Span::from_sqlparser(&column_ident.span);

        if let Some(table_id) = table_ident {
            let table_alias = &table_id.value;
            // Qualified column reference (table.column)
            if let Some(table_ref) = self.tables.get(table_alias) {
                // Check derived table first
                if let Some(derived_cols) = &table_ref.derived_columns {
                    if !derived_cols
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(column_name))
                        && !derived_cols.iter().any(|c| c.starts_with("?column?"))
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
                } else if let Some(cte) = self.ctes.get(&table_ref.table.name) {
                    // Validate against CTE columns
                    if !cte.columns.contains(column_name) {
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
                    // Validate against VIEW columns
                    if !view_cols
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
                    if !table_def.column_exists(column_name) {
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
            // Unqualified column reference - search all tables in scope
            let mut found_in: Vec<&str> = Vec::new();

            for (name, table_ref) in &self.tables {
                // Check derived table first
                if let Some(derived_cols) = &table_ref.derived_columns {
                    if derived_cols
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(column_name))
                    {
                        found_in.push(name);
                    }
                } else if let Some(cte) = self.ctes.get(&table_ref.table.name) {
                    // Check CTE
                    if cte.columns.contains(column_name) {
                        found_in.push(name);
                    }
                } else if let Some(view_cols) = &table_ref.view_columns {
                    // Check VIEW columns
                    if view_cols
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(column_name))
                    {
                        found_in.push(name);
                    }
                } else if let Some(table_def) = self.catalog.get_table(&table_ref.table) {
                    if table_def.column_exists(column_name) {
                        found_in.push(name);
                    }
                }
            }

            match found_in.len() {
                0 => {
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

    /// Consume the resolver and return collected diagnostics
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

/// Convert ObjectName to QualifiedName
fn object_name_to_qualified(name: &ObjectName) -> QualifiedName {
    match name.0.as_slice() {
        [table] => QualifiedName::new(&table.value),
        [schema, table] => QualifiedName::with_schema(&schema.value, &table.value),
        [_catalog, schema, table] => QualifiedName::with_schema(&schema.value, &table.value),
        _ => QualifiedName::new(name.to_string()),
    }
}

/// Get table name from TableFactor
fn table_with_joins_to_name(factor: &TableFactor) -> Option<QualifiedName> {
    match factor {
        TableFactor::Table { name, .. } => Some(object_name_to_qualified(name)),
        _ => None,
    }
}

/// Find a similar column name (for suggestions)
fn find_similar_column(table: &TableDef, name: &str) -> Option<String> {
    let name_lower = name.to_lowercase();
    let mut best_match: Option<(usize, &str)> = None;

    for col_name in table.columns.keys() {
        let col_lower = col_name.to_lowercase();
        let distance = levenshtein_distance(&name_lower, &col_lower);

        // Only suggest if reasonably similar (distance <= 3)
        if distance <= 3 && (best_match.is_none() || distance < best_match.unwrap().0) {
            best_match = Some((distance, col_name));
        }
    }

    best_match.map(|(_, name)| name.to_string())
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
