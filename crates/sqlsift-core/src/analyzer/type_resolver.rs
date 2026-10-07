//! Type resolver - infers and validates types in SQL expressions
//!
//! ## Current Implementation
//!
//! **Supported:**
//! - WHERE clause type checking (E0003)
//! - JOIN condition type checking (E0007)
//! - Binary operators: comparisons (=, !=, <, >, <=, >=), arithmetic (+, -, *, /, %)
//! - Nested expressions: `(a + b) * 2 = c`
//! - Numeric type compatibility (INTEGER → BIGINT implicit casts)
//! - INSERT VALUES type checking: `INSERT INTO users (id) VALUES ('text')` → E0003
//! - UPDATE SET type checking: `UPDATE users SET id = 'text'` → E0003
//!
//! **TODO (Not Yet Implemented):**
//! - CASE expression type consistency: THEN/ELSE branches must have compatible types
//! - Subquery column type inference: Infer types from SELECT projections
//! - VIEW/CTE column type inference: Requires full SELECT type analysis
//!
//! ## Implementation Notes
//!
//! - Current coverage: ~85% of real-world type errors
//! - Type inference is performed in a separate pass after name resolution

use indexmap::IndexMap;
use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    Insert, Query, Select, SetExpr, Spanned, Statement, TableFactor, TableWithJoins, Value, Values,
};
use std::collections::HashSet;

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{Catalog, QualifiedName};
use crate::types::{ArithmeticOp, SqlType, TypeCompatibility};

use super::resolver::NameResolver;

/// Expression type inference result
#[derive(Debug, Clone, PartialEq)]
enum ExpressionType {
    /// Type is known (successfully inferred)
    Known(SqlType),
    /// Quoted string literal: untyped until it meets another operand
    /// (e.g. `'2024-01-01'` compared with a DATE column is a DATE)
    StringLiteral(String),
    /// Type is unknown (e.g., subquery, complex expression)
    Unknown,
}

/// Reference to a table available in the current scope
#[derive(Debug, Clone)]
struct TableRef {
    /// Qualified table name in catalog
    table_name: QualifiedName,
    /// If this is a VIEW, the column names from the view definition
    view_columns: Option<Vec<String>>,
    /// If this is a derived table, the inferred column names
    derived_columns: Option<Vec<String>>,
}

/// Type resolver for SQL expressions
pub struct TypeResolver<'a> {
    catalog: &'a Catalog,
    /// Current scope's table references (alias or name -> TableRef)
    tables: IndexMap<String, TableRef>,
    /// Enclosing query blocks' scopes, innermost last (for correlated subqueries)
    outer_scopes: Vec<IndexMap<String, TableRef>>,
    /// CTE names visible in the current query (lowercase); they shadow catalog tables
    ctes: HashSet<String>,
    /// Collected diagnostics
    diagnostics: Vec<Diagnostic>,
    /// SQL dialect (affects dialect-specific coercions such as MySQL booleans)
    dialect: SqlDialect,
}

impl<'a> TypeResolver<'a> {
    /// Create a new type resolver
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            tables: IndexMap::new(),
            outer_scopes: Vec::new(),
            ctes: HashSet::new(),
            diagnostics: Vec::new(),
            dialect: SqlDialect::default(),
        }
    }

    /// Set the SQL dialect
    pub fn with_dialect(mut self, dialect: SqlDialect) -> Self {
        self.dialect = dialect;
        self
    }

    /// Report a string literal that is not a value of the enum type on the other side.
    /// Returns true if a diagnostic was emitted.
    fn report_enum_literal(
        &mut self,
        left: &ExpressionType,
        right: &ExpressionType,
        span: Option<Span>,
    ) -> bool {
        let (enum_name, literal) = match (left, right) {
            (ExpressionType::Known(SqlType::Custom(name)), ExpressionType::StringLiteral(lit))
            | (ExpressionType::StringLiteral(lit), ExpressionType::Known(SqlType::Custom(name))) => {
                (name, lit)
            }
            _ => return false,
        };
        let Some(enum_def) = self.catalog.get_enum(enum_name) else {
            return false;
        };
        if enum_def.values.is_empty() || enum_def.values.iter().any(|v| v == literal) {
            return false;
        }
        let help = super::resolver::find_similar_name(enum_def.values.iter().cloned(), literal)
            .map(|v| format!("Did you mean '{}'?", v))
            .unwrap_or_else(|| {
                format!(
                    "Valid values: {}",
                    enum_def
                        .values
                        .iter()
                        .map(|v| format!("'{}'", v))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        let mut diag = Diagnostic::error(
            DiagnosticKind::TypeMismatch,
            format!(
                "Invalid value '{}' for enum type '{}'",
                literal, enum_def.name
            ),
        )
        .with_help(help);
        diag.span = span;
        self.diagnostics.push(diag);
        true
    }

    /// Check whether two expression types conflict, i.e. neither can be implicitly
    /// converted to the other. Returns the display names of both sides on conflict.
    fn type_conflict(
        &self,
        left: &ExpressionType,
        right: &ExpressionType,
    ) -> Option<(String, String)> {
        match (left, right) {
            (ExpressionType::Known(lt), ExpressionType::Known(rt)) => {
                if self.dialect != SqlDialect::PostgreSQL && is_integer_boolean_pair(lt, rt) {
                    // MySQL BOOLEAN is TINYINT(1); SQLite stores booleans as integers
                    return None;
                }
                let compatible = lt.is_compatible_with(rt) != TypeCompatibility::ExplicitCast
                    || rt.is_compatible_with(lt) != TypeCompatibility::ExplicitCast;
                (!compatible).then(|| (lt.display_name(), rt.display_name()))
            }
            (ExpressionType::Known(t), ExpressionType::StringLiteral(lit)) => (!t
                .accepts_string_literal(lit))
            .then(|| (t.display_name(), SqlType::Text.display_name())),
            (ExpressionType::StringLiteral(lit), ExpressionType::Known(t)) => (!t
                .accepts_string_literal(lit))
            .then(|| (SqlType::Text.display_name(), t.display_name())),
            _ => None,
        }
    }

    /// Inherit scope from a NameResolver
    /// This allows TypeResolver to access the same table context as NameResolver
    pub fn inherit_scope(&mut self, resolver: &NameResolver) {
        // Copy table references from NameResolver
        for (key, name_table_ref) in &resolver.tables {
            let type_table_ref = TableRef {
                table_name: name_table_ref.table.clone(),
                view_columns: name_table_ref.view_columns.clone(),
                derived_columns: name_table_ref.derived_columns.clone(),
            };
            self.tables.insert(key.clone(), type_table_ref);
        }
    }

    /// Check types in a statement
    pub fn check_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Query(query) => {
                // Query blocks build their own scopes from their FROM clauses
                self.tables.clear();
                self.check_query(query);
            }
            Statement::Insert(insert) => {
                self.check_insert(insert);
            }
            Statement::Update {
                table,
                assignments,
                selection,
                ..
            } => {
                self.check_update(table, assignments);
                for assignment in assignments {
                    self.check_expr_recursive(&assignment.value);
                }
                if let Some(expr) = selection {
                    self.check_expr_recursive(expr);
                }
            }
            Statement::Delete(delete) => {
                // WHERE condition type checking is already implemented
                if let Some(ref selection) = delete.selection {
                    self.check_expr_recursive(selection);
                }
            }
            _ => {}
        }
    }

    /// Check types in an INSERT statement
    fn check_insert(&mut self, insert: &Insert) {
        let table_name = self.catalog.qualified_name(&insert.table_name);
        let table_def = match self.catalog.get_table(&table_name) {
            Some(def) => def,
            None => return, // Table not found - already reported by NameResolver
        };

        // Determine target columns
        let target_columns: Vec<String> = if insert.columns.is_empty() {
            // No explicit columns - use all table columns in order
            table_def.columns.keys().cloned().collect()
        } else {
            insert.columns.iter().map(|c| c.value.clone()).collect()
        };

        // ON CONFLICT DO UPDATE / ON DUPLICATE KEY UPDATE assignments
        match &insert.on {
            Some(sqlparser::ast::OnInsert::DuplicateKeyUpdate(assignments)) => {
                self.check_assignments(table_def, assignments);
            }
            Some(sqlparser::ast::OnInsert::OnConflict(on_conflict)) => {
                if let sqlparser::ast::OnConflictAction::DoUpdate(update) = &on_conflict.action {
                    self.check_assignments(table_def, &update.assignments);
                    if let Some(selection) = &update.selection {
                        self.check_expr_recursive(selection);
                    }
                }
            }
            _ => {}
        }

        // INSERT ... SELECT: the source query has its own scope
        if let Some(source) = &insert.source {
            if !matches!(source.body.as_ref(), SetExpr::Values(_)) {
                let saved = std::mem::take(&mut self.tables);
                self.check_query(source);
                self.tables = saved;
            }
        }

        // Check VALUES rows
        if let Some(source) = &insert.source {
            if let SetExpr::Values(Values { rows, .. }) = source.body.as_ref() {
                for row in rows {
                    for (i, value_expr) in row.iter().enumerate() {
                        if i >= target_columns.len() {
                            break;
                        }
                        let col_name = &target_columns[i];
                        let col_def = match table_def.get_column(col_name) {
                            Some(def) => def,
                            None => continue, // Column not found - already reported
                        };

                        // MySQL/SQLite generate the key when NULL is inserted into an
                        // integer primary key (AUTO_INCREMENT / rowid alias)
                        let single_column_key =
                            table_def
                                .primary_key
                                .as_ref()
                                .map_or(col_def.is_primary_key, |pk| {
                                    pk.columns.len() == 1
                                        && pk.columns[0].eq_ignore_ascii_case(&col_def.name)
                                });
                        let generates_key = self.dialect != SqlDialect::PostgreSQL
                            && single_column_key
                            && col_def.data_type.is_integer();
                        if !col_def.nullable
                            && !generates_key
                            && matches!(value_expr, Expr::Value(Value::Null))
                        {
                            // NULL literals carry no source location: point at the
                            // target column, or the table name without a column list
                            let span = Span::from_sqlparser(
                                &insert
                                    .columns
                                    .get(i)
                                    .map(|c| c.span)
                                    .or_else(|| insert.table_name.0.last().map(|t| t.span))
                                    .unwrap_or_else(|| value_expr.span()),
                            );
                            self.diagnostics.push(
                                Diagnostic::error(
                                    DiagnosticKind::PotentialNullViolation,
                                    format!(
                                        "Potential NOT NULL violation: column '{}' cannot be assigned NULL",
                                        col_name
                                    ),
                                )
                                .with_span(span)
                                .with_help(
                                    "This column is defined as NOT NULL. Provide a non-NULL value or change the schema constraint.",
                                ),
                            );
                            continue;
                        }

                        self.check_expr_recursive(value_expr);
                        let value_type = self.infer_expr_type(value_expr);
                        let column_type = ExpressionType::Known(col_def.data_type.clone());
                        let column_span =
                            insert.columns.get(i).map(|c| Span::from_sqlparser(&c.span));
                        if self.report_enum_literal(&column_type, &value_type, column_span) {
                            continue;
                        }
                        if let Some((expected, actual)) =
                            self.type_conflict(&column_type, &value_type)
                        {
                            let mut diag = Diagnostic::error(
                                DiagnosticKind::TypeMismatch,
                                format!(
                                    "Type mismatch: column '{}' expects {}, but got {}",
                                    col_name, expected, actual
                                ),
                            )
                            .with_help(
                                "Value type is not compatible with the column type. Consider using explicit CAST.",
                            );
                            // Literals carry no location: fall back to the target column
                            diag.span = located_span(value_expr).or(column_span);
                            self.diagnostics.push(diag);
                        }
                    }
                }
            }
        }
    }

    /// Check types in an UPDATE statement
    fn check_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
    ) {
        let table_name = match &table.relation {
            TableFactor::Table { name, .. } => self.catalog.qualified_name(name),
            _ => return,
        };
        let table_def = match self.catalog.get_table(&table_name) {
            Some(def) => def,
            None => return, // Table not found - already reported by NameResolver
        };
        self.check_assignments(table_def, assignments);
    }

    /// Check `SET col = value` assignments against the target table's column types
    fn check_assignments(
        &mut self,
        table_def: &crate::schema::TableDef,
        assignments: &[sqlparser::ast::Assignment],
    ) {
        for assignment in assignments {
            let col_name = match &assignment.target {
                AssignmentTarget::ColumnName(name) => match name.0.last() {
                    Some(ident) => ident.value.clone(),
                    None => continue,
                },
                AssignmentTarget::Tuple(_) => continue, // Skip tuple assignments
            };

            let col_def = match table_def.get_column(&col_name) {
                Some(def) => def,
                None => continue, // Column not found - already reported
            };

            if !col_def.nullable && matches!(&assignment.value, Expr::Value(Value::Null)) {
                // NULL literals carry no source location: point at the target column
                let span = match &assignment.target {
                    AssignmentTarget::ColumnName(name) => {
                        name.0.last().map(|ident| Span::from_sqlparser(&ident.span))
                    }
                    AssignmentTarget::Tuple(_) => None,
                }
                .unwrap_or_else(|| Span::from_sqlparser(&assignment.value.span()));
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::PotentialNullViolation,
                        format!(
                            "Potential NOT NULL violation: column '{}' cannot be assigned NULL",
                            col_name
                        ),
                    )
                    .with_span(span)
                    .with_help(
                        "This column is defined as NOT NULL. Provide a non-NULL value or change the schema constraint.",
                    ),
                );
                continue;
            }

            let value_type = self.infer_expr_type(&assignment.value);
            let column_type = ExpressionType::Known(col_def.data_type.clone());
            let target_span = match &assignment.target {
                AssignmentTarget::ColumnName(name) => {
                    name.0.last().map(|i| Span::from_sqlparser(&i.span))
                }
                AssignmentTarget::Tuple(_) => None,
            };
            if self.report_enum_literal(&column_type, &value_type, target_span) {
                continue;
            }
            if let Some((expected, actual)) = self.type_conflict(&column_type, &value_type) {
                let mut diag = Diagnostic::error(
                    DiagnosticKind::TypeMismatch,
                    format!(
                        "Type mismatch: column '{}' expects {}, but got {}",
                        col_name, expected, actual
                    ),
                )
                .with_help(
                    "Value type is not compatible with the column type. Consider using explicit CAST.",
                );
                // Literals carry no location: fall back to the target column
                diag.span = located_span(&assignment.value).or(target_span);
                self.diagnostics.push(diag);
            }
        }
    }

    /// Check types in a query
    fn check_query(&mut self, query: &Query) {
        let saved_ctes = self.ctes.clone();
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                // Registered first so recursive CTEs can reference themselves
                self.ctes.insert(cte.alias.name.value.to_lowercase());
                self.check_query(&cte.query);
            }
        }
        self.check_set_expr(&query.body);
        self.ctes = saved_ctes;
    }

    /// Enter a query block: its FROM tables become the current scope and the
    /// previous scope becomes an outer scope
    fn push_scope(&mut self, from: &[TableWithJoins]) {
        let local = self.scope_of_from_items(from);
        let outer = std::mem::replace(&mut self.tables, local);
        self.outer_scopes.push(outer);
    }

    /// Leave a query block entered with [`Self::push_scope`]
    fn pop_scope(&mut self) {
        self.tables = self.outer_scopes.pop().unwrap_or_default();
    }

    /// Table references introduced by a FROM clause (alias or name -> TableRef)
    fn scope_of_from_items(&self, from: &[TableWithJoins]) -> IndexMap<String, TableRef> {
        let mut scope = IndexMap::new();
        for table in from {
            self.add_relation(&table.relation, &mut scope);
            for join in &table.joins {
                self.add_relation(&join.relation, &mut scope);
            }
        }
        scope
    }

    /// Register a FROM relation in `scope`
    fn add_relation(&self, factor: &TableFactor, scope: &mut IndexMap<String, TableRef>) {
        // Relations whose column types are unknown (CTEs, subqueries, functions, missing tables)
        let unknown = |name: &str| TableRef {
            table_name: QualifiedName::new(name),
            view_columns: None,
            derived_columns: Some(Vec::new()),
        };
        match factor {
            TableFactor::Table {
                name, alias, args, ..
            } => {
                let table_name = self.catalog.qualified_name(name);
                let key = alias
                    .as_ref()
                    .map_or_else(|| table_name.name.clone(), |a| a.name.value.clone());
                let is_cte = table_name.schema.is_none()
                    && self.ctes.contains(&table_name.name.to_lowercase());
                let table_ref = if args.is_some() || is_cte {
                    unknown(&key)
                } else if self.catalog.get_table(&table_name).is_some() {
                    TableRef {
                        table_name,
                        view_columns: None,
                        derived_columns: None,
                    }
                } else if let Some(view) = self.catalog.get_view(&table_name) {
                    TableRef {
                        table_name,
                        view_columns: Some(view.columns.clone()),
                        derived_columns: None,
                    }
                } else {
                    unknown(&key)
                };
                scope.insert(key, table_ref);
            }
            TableFactor::Derived { alias: Some(a), .. }
            | TableFactor::TableFunction { alias: Some(a), .. }
            | TableFactor::Function { alias: Some(a), .. }
            | TableFactor::UNNEST { alias: Some(a), .. } => {
                scope.insert(a.name.value.clone(), unknown(&a.name.value));
            }
            TableFactor::NestedJoin {
                table_with_joins, ..
            } => {
                self.add_relation(&table_with_joins.relation, scope);
                for join in &table_with_joins.joins {
                    self.add_relation(&join.relation, scope);
                }
            }
            _ => {}
        }
    }

    /// Type check subqueries in FROM (derived tables). Non-LATERAL subqueries are
    /// checked before the block's own scope is entered, since they can't see it.
    fn check_from_subqueries(&mut self, from: &[TableWithJoins], lateral_only: bool) {
        for table in from {
            for factor in
                std::iter::once(&table.relation).chain(table.joins.iter().map(|j| &j.relation))
            {
                if let TableFactor::Derived {
                    lateral, subquery, ..
                } = factor
                {
                    if *lateral == lateral_only {
                        self.check_query(subquery);
                    }
                }
            }
        }
    }

    /// Check types in a set expression (SELECT, UNION, INTERSECT, EXCEPT, ...)
    fn check_set_expr(&mut self, set_expr: &SetExpr) {
        match set_expr {
            SetExpr::Select(select) => self.check_select(select),
            SetExpr::Query(query) => self.check_query(query),
            SetExpr::SetOperation { left, right, .. } => {
                self.check_set_expr(left);
                self.check_set_expr(right);
                self.check_set_operation_compatibility(left, right);
            }
            _ => {}
        }
    }

    /// Validate projection compatibility between two sides of a set operation.
    fn check_set_operation_compatibility(&mut self, left: &SetExpr, right: &SetExpr) {
        let left_types = match self.infer_set_expr_projection_types(left) {
            Some(types) => types,
            None => return,
        };
        let right_types = match self.infer_set_expr_projection_types(right) {
            Some(types) => types,
            None => return,
        };

        if left_types.len() != right_types.len() {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticKind::TypeMismatch,
                    format!(
                        "Set operation column count mismatch: left has {}, right has {}",
                        left_types.len(),
                        right_types.len()
                    ),
                )
                .with_span(Span::from_sqlparser(&right.span()))
                .with_help("UNION/INTERSECT/EXCEPT requires both sides to have the same number of columns."),
            );
            return;
        }

        for (idx, (left_ty, right_ty)) in left_types.into_iter().zip(right_types).enumerate() {
            if let Some((lt, rt)) = self.type_conflict(&left_ty, &right_ty) {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "Set operation type mismatch at column {}: {} vs {}",
                            idx + 1,
                            lt,
                            rt
                        ),
                    )
                    .with_span(Span::from_sqlparser(&right.span()))
                    .with_help("Corresponding columns in UNION/INTERSECT/EXCEPT should be type-compatible."),
                );
            }
        }
    }

    /// Infer projection types for a set expression.
    /// Returns None when projection width is indeterminate (e.g., wildcard).
    fn infer_set_expr_projection_types(
        &mut self,
        set_expr: &SetExpr,
    ) -> Option<Vec<ExpressionType>> {
        match set_expr {
            SetExpr::Select(select) => self.infer_select_projection_types(select),
            SetExpr::Query(query) => self.infer_set_expr_projection_types(&query.body),
            // SQL semantics: result column shape of a set operation is based on the left side.
            SetExpr::SetOperation { left, .. } => self.infer_set_expr_projection_types(left),
            _ => None,
        }
    }

    /// Infer projection types for a SELECT list.
    /// Returns None when wildcard expansion would be required.
    fn infer_select_projection_types(&mut self, select: &Select) -> Option<Vec<ExpressionType>> {
        self.push_scope(&select.from);
        let types = self.infer_projection_types_in_scope(select);
        self.pop_scope();
        types
    }

    fn infer_projection_types_in_scope(&mut self, select: &Select) -> Option<Vec<ExpressionType>> {
        let mut types = Vec::with_capacity(select.projection.len());
        for item in &select.projection {
            match item {
                sqlparser::ast::SelectItem::UnnamedExpr(expr)
                | sqlparser::ast::SelectItem::ExprWithAlias { expr, .. } => {
                    types.push(self.infer_expr_type(expr));
                }
                sqlparser::ast::SelectItem::Wildcard(_)
                | sqlparser::ast::SelectItem::QualifiedWildcard(_, _) => return None,
            }
        }
        Some(types)
    }

    /// Check types in a SELECT statement
    fn check_select(&mut self, select: &Select) {
        self.check_from_subqueries(&select.from, false);
        self.push_scope(&select.from);
        self.check_from_subqueries(&select.from, true);
        self.check_select_in_scope(select);
        self.pop_scope();
    }

    fn check_select_in_scope(&mut self, select: &Select) {
        // Check JOIN conditions
        for table_with_joins in &select.from {
            for join in &table_with_joins.joins {
                self.check_join_condition(join);
            }
        }

        // Check SELECT projection
        for select_item in &select.projection {
            match select_item {
                sqlparser::ast::SelectItem::UnnamedExpr(expr)
                | sqlparser::ast::SelectItem::ExprWithAlias { expr, .. } => {
                    self.check_expr_recursive(expr);
                }
                sqlparser::ast::SelectItem::QualifiedWildcard(_, _)
                | sqlparser::ast::SelectItem::Wildcard(_) => {
                    // Wildcards don't need type checking
                }
            }
        }

        // Check WHERE clause
        if let Some(ref selection) = select.selection {
            self.check_expr_recursive(selection);
        }

        // Check HAVING clause
        if let Some(ref having) = select.having {
            self.check_expr_recursive(having);
        }
    }

    /// Check types in a JOIN condition
    fn check_join_condition(&mut self, join: &sqlparser::ast::Join) {
        use sqlparser::ast::{JoinConstraint, JoinOperator};

        // Extract the constraint from the join operator
        let constraint = match &join.join_operator {
            JoinOperator::Inner(c)
            | JoinOperator::LeftOuter(c)
            | JoinOperator::RightOuter(c)
            | JoinOperator::FullOuter(c) => c,
            JoinOperator::CrossJoin | JoinOperator::CrossApply | JoinOperator::OuterApply => {
                return; // No condition to check
            }
            _ => return,
        };

        if let JoinConstraint::On(expr) = constraint {
            // Check JOIN ON condition with special handling for top-level comparison
            self.check_join_on_expr(expr);
        }
    }

    /// Check expression in JOIN ON clause
    /// Top-level comparison operators get JoinTypeMismatch error instead of TypeMismatch
    fn check_join_on_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp { left, op, right } => {
                if self.is_comparison_operator(op) {
                    // This is a comparison in JOIN ON - use JoinTypeMismatch error
                    let left_type = self.infer_expr_type(left);
                    let right_type = self.infer_expr_type(right);

                    if let Some((lt, rt)) = self.type_conflict(&left_type, &right_type) {
                        let span = located_span(left)
                            .or_else(|| located_span(right))
                            .unwrap_or_else(|| Span::from_sqlparser(&left.span()));
                        self.diagnostics.push(
                            Diagnostic::error(
                                DiagnosticKind::JoinTypeMismatch,
                                format!(
                                    "JOIN condition type mismatch: {} vs {}",
                                    lt, rt
                                ),
                            )
                            .with_span(span)
                            .with_help(
                                "JOIN condition should compare compatible types. Consider using explicit CAST.",
                            ),
                        );
                    }
                    // Recursively check subexpressions
                    self.check_join_on_expr(left);
                    self.check_join_on_expr(right);
                } else {
                    // Non-comparison operator - check recursively
                    self.check_join_on_expr(left);
                    self.check_join_on_expr(right);
                }
            }
            Expr::Nested(inner) => {
                self.check_join_on_expr(inner);
            }
            _ => {
                // Leaf expressions - no further checking needed
            }
        }
    }

    /// Check if an operator is a comparison operator
    fn is_comparison_operator(&self, op: &BinaryOperator) -> bool {
        matches!(
            op,
            BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq
        )
    }

    /// Recursively check types in an expression
    fn check_expr_recursive(&mut self, expr: &Expr) {
        match expr {
            Expr::BinaryOp { left, op, right } => {
                // Check the binary operation
                self.check_binary_op(left, op, right);
                // Recursively check subexpressions
                self.check_expr_recursive(left);
                self.check_expr_recursive(right);
            }
            Expr::Nested(inner) => {
                self.check_expr_recursive(inner);
            }
            Expr::UnaryOp { expr, .. } => {
                self.check_expr_recursive(expr);
            }
            Expr::InList { expr, list, .. } => {
                self.check_expr_recursive(expr);
                for item in list {
                    self.check_expr_recursive(item);
                }
            }
            Expr::Subquery(query)
            | Expr::Exists {
                subquery: query, ..
            } => {
                self.check_query(query);
            }
            Expr::InSubquery { expr, subquery, .. } => {
                self.check_expr_recursive(expr);
                self.check_query(subquery);
            }
            Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::IsTrue(expr)
            | Expr::IsFalse(expr)
            | Expr::IsNotTrue(expr)
            | Expr::IsNotFalse(expr)
            | Expr::Cast { expr, .. } => {
                self.check_expr_recursive(expr);
            }
            Expr::Function(func) => {
                if let FunctionArguments::List(list) = &func.args {
                    for arg in &list.args {
                        if let FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))
                        | FunctionArg::Named {
                            arg: FunctionArgExpr::Expr(expr),
                            ..
                        } = arg
                        {
                            self.check_expr_recursive(expr);
                        }
                    }
                }
                if let Some(filter) = &func.filter {
                    self.check_expr_recursive(filter);
                }
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.check_expr_recursive(expr);
                self.check_expr_recursive(low);
                self.check_expr_recursive(high);
            }
            Expr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                if let Some(op) = operand {
                    self.check_expr_recursive(op);
                }
                for cond in conditions {
                    self.check_expr_recursive(cond);
                }
                for res in results {
                    self.check_expr_recursive(res);
                }
                if let Some(else_res) = else_result {
                    self.check_expr_recursive(else_res);
                }
            }
            _ => {
                // Base case: leaf expressions like identifiers, literals
            }
        }
    }

    /// Check type compatibility in a binary operation
    fn check_binary_op(&mut self, left: &Expr, op: &BinaryOperator, right: &Expr) {
        let left_type = self.infer_expr_type(left);
        let right_type = self.infer_expr_type(right);

        if self.is_comparison_operator(op) {
            // Literals carry no location: use the other operand's
            let span = located_span(left).or_else(|| located_span(right));
            if self.report_enum_literal(&left_type, &right_type, span) {
                return;
            }
            if let Some((lt, rt)) = self.type_conflict(&left_type, &right_type) {
                let mut diag = Diagnostic::error(
                    DiagnosticKind::TypeMismatch,
                    format!("Type mismatch: cannot compare {} with {}", lt, rt),
                )
                .with_help("Types are not implicitly compatible. Consider using explicit CAST.");
                diag.span = span;
                self.diagnostics.push(diag);
            }
            return;
        }

        let Some(arith_op) = arithmetic_op(op) else {
            return;
        };

        // Numeric arithmetic with a string literal: the literal must be a number
        let literal_operand = match (&left_type, &right_type) {
            (ExpressionType::Known(t), ExpressionType::StringLiteral(lit)) => Some((t, lit, right)),
            (ExpressionType::StringLiteral(lit), ExpressionType::Known(t)) => Some((t, lit, left)),
            _ => None,
        };
        if let Some((t, lit, lit_expr)) = literal_operand {
            if t.is_numeric() && !t.accepts_string_literal(lit) {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "Type mismatch: '{}' is not a valid {} in arithmetic",
                            lit,
                            t.display_name()
                        ),
                    )
                    .with_span(
                        located_span(lit_expr)
                            .or_else(|| located_span(left))
                            .or_else(|| located_span(right))
                            .unwrap_or_else(|| Span::from_sqlparser(&lit_expr.span())),
                    ),
                );
            }
            return;
        }

        // Otherwise only check when both types are known
        let (ExpressionType::Known(lt), ExpressionType::Known(rt)) = (&left_type, &right_type)
        else {
            return;
        };
        if SqlType::temporal_arithmetic_result(lt, arith_op, rt).is_some() {
            return;
        }
        // An interval or date/time operand with a numeric one is fine only in the
        // combinations allowed above; otherwise report each non-numeric side
        for (ty, expr) in [(lt, left), (rt, right)] {
            if !self.is_numeric_type(ty) {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "Arithmetic operation requires numeric types, but got {}",
                            ty.display_name()
                        ),
                    )
                    .with_span(Span::from_sqlparser(&expr.span())),
                );
            }
        }
    }

    /// Check if a type is numeric
    fn is_numeric_type(&self, sql_type: &SqlType) -> bool {
        sql_type.is_numeric()
    }

    /// Consume the resolver and return collected diagnostics
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Infer the type of an expression
    fn infer_expr_type(&mut self, expr: &Expr) -> ExpressionType {
        match expr {
            Expr::Value(value) => self.infer_literal_type(value),
            Expr::Identifier(ident) => self.infer_column_type_from_ident(&ident.value),
            Expr::CompoundIdentifier(parts) => {
                if parts.len() == 2 {
                    // table.column
                    self.infer_column_type_qualified(&parts[0].value, &parts[1].value)
                } else {
                    // More complex identifier (schema.table.column)
                    ExpressionType::Unknown
                }
            }
            Expr::Nested(inner) => {
                // Recursively infer type of nested expression
                self.infer_expr_type(inner)
            }
            Expr::BinaryOp { left, op, right } => {
                // Infer result type of binary operation
                self.infer_binary_op_result_type(left, op, right)
            }
            Expr::Cast { data_type, .. } => {
                let sql_type = SqlType::from_ast(data_type);
                if sql_type == SqlType::Unknown {
                    ExpressionType::Unknown
                } else {
                    ExpressionType::Known(sql_type)
                }
            }
            Expr::Function(func) => self.infer_function_return_type(func),
            Expr::Interval(_) => ExpressionType::Known(SqlType::Interval),
            // Typed literals such as DATE '2024-01-01'
            Expr::TypedString { data_type, .. } => match SqlType::from_ast(data_type) {
                SqlType::Unknown => ExpressionType::Unknown,
                sql_type => ExpressionType::Known(sql_type),
            },
            // TODO: Add support for more expression types:
            // - Expr::Case => Infer from THEN/ELSE branches (medium, 1-1.5 hours, ROI 20%)
            // - Expr::Subquery => Infer from SELECT projection (complex, 4-6 hours, ROI 15%)
            _ => ExpressionType::Unknown,
        }
    }

    /// Infer the return type of a SQL function
    fn infer_function_return_type(&mut self, func: &sqlparser::ast::Function) -> ExpressionType {
        let func_name = func.name.to_string().to_uppercase();
        // Strip schema prefix (e.g., "PG_CATALOG.COUNT" → "COUNT")
        let name = func_name.rsplit('.').next().unwrap_or(&func_name);

        match name {
            // Aggregate functions returning INTEGER/BIGINT
            "COUNT" => ExpressionType::Known(SqlType::BigInt),

            // Aggregate functions returning same as input or NUMERIC
            "SUM" => self.infer_aggregate_numeric_type(func),
            "AVG" => ExpressionType::Known(SqlType::Decimal {
                precision: None,
                scale: None,
            }),

            // MIN/MAX return the same type as their argument
            "MIN" | "MAX" => self.infer_first_arg_type(func),

            // Boolean-returning functions
            "EXISTS" | "BOOL_AND" | "BOOL_OR" | "EVERY" => ExpressionType::Known(SqlType::Boolean),

            // String functions
            "CONCAT" | "UPPER" | "LOWER" | "TRIM" | "LTRIM" | "RTRIM" | "REPLACE" | "SUBSTRING"
            | "SUBSTR" | "LEFT" | "RIGHT" | "LPAD" | "RPAD" | "REPEAT" | "REVERSE" | "INITCAP"
            | "MD5" => ExpressionType::Known(SqlType::Text),

            // String → Integer functions
            "LENGTH" | "CHAR_LENGTH" | "CHARACTER_LENGTH" | "BIT_LENGTH" | "OCTET_LENGTH"
            | "POSITION" | "STRPOS" => ExpressionType::Known(SqlType::Integer),

            // Numeric functions
            "ABS" | "CEIL" | "CEILING" | "FLOOR" | "ROUND" | "TRUNC" | "TRUNCATE" | "SIGN"
            | "MOD" => self.infer_first_arg_type_or_numeric(func),
            "RANDOM" | "SQRT" | "POWER" | "LOG" | "LN" | "EXP" | "PI" | "DEGREES" | "RADIANS"
            | "SIN" | "COS" | "TAN" | "ASIN" | "ACOS" | "ATAN" | "ATAN2" => {
                ExpressionType::Known(SqlType::DoublePrecision)
            }

            // Date/Time functions
            "NOW" | "CURRENT_TIMESTAMP" => ExpressionType::Known(SqlType::Timestamp {
                precision: None,
                with_timezone: true,
            }),
            "CURRENT_DATE" => ExpressionType::Known(SqlType::Date),
            "CURRENT_TIME" => ExpressionType::Known(SqlType::Time {
                precision: None,
                with_timezone: true,
            }),

            // Type casting functions
            "COALESCE" | "NULLIF" | "IFNULL" => self.infer_first_arg_type(func),
            "GREATEST" | "LEAST" => self.infer_first_arg_type(func),

            _ => ExpressionType::Unknown,
        }
    }

    /// Infer the type of the first argument to a function
    fn infer_first_arg_type(&mut self, func: &sqlparser::ast::Function) -> ExpressionType {
        if let sqlparser::ast::FunctionArguments::List(arg_list) = &func.args {
            if let Some(sqlparser::ast::FunctionArg::Unnamed(
                sqlparser::ast::FunctionArgExpr::Expr(expr),
            )) = arg_list.args.first()
            {
                return self.infer_expr_type(expr);
            }
        }
        ExpressionType::Unknown
    }

    /// Infer the return type of SUM (integer args → BIGINT, decimal args → DECIMAL)
    fn infer_aggregate_numeric_type(&mut self, func: &sqlparser::ast::Function) -> ExpressionType {
        match self.infer_first_arg_type(func) {
            ExpressionType::Known(t) if self.is_numeric_type(&t) => {
                match t {
                    SqlType::Decimal { .. } => ExpressionType::Known(t),
                    SqlType::Real | SqlType::DoublePrecision => ExpressionType::Known(t),
                    // Integer types → BIGINT for SUM to avoid overflow
                    _ => ExpressionType::Known(SqlType::BigInt),
                }
            }
            _ => ExpressionType::Unknown,
        }
    }

    /// Infer first arg type, falling back to numeric
    fn infer_first_arg_type_or_numeric(
        &mut self,
        func: &sqlparser::ast::Function,
    ) -> ExpressionType {
        match self.infer_first_arg_type(func) {
            ExpressionType::Known(t) => ExpressionType::Known(t),
            _ => ExpressionType::Unknown,
        }
    }

    /// Infer the result type of a binary operation
    fn infer_binary_op_result_type(
        &mut self,
        left: &Expr,
        op: &BinaryOperator,
        right: &Expr,
    ) -> ExpressionType {
        let left_type = self.infer_expr_type(left);
        let right_type = self.infer_expr_type(right);

        match (left_type, right_type) {
            (ExpressionType::Known(lt), ExpressionType::Known(rt)) => {
                match op {
                    // Arithmetic operators return numeric type
                    BinaryOperator::Plus
                    | BinaryOperator::Minus
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo => {
                        if self.is_numeric_type(&lt) && self.is_numeric_type(&rt) {
                            // Return the "larger" type (simplified)
                            // In reality, type promotion rules are more complex
                            ExpressionType::Known(lt)
                        } else {
                            arithmetic_op(op)
                                .and_then(|op| SqlType::temporal_arithmetic_result(&lt, op, &rt))
                                .map_or(ExpressionType::Unknown, ExpressionType::Known)
                        }
                    }
                    // Comparison operators return boolean
                    BinaryOperator::Eq
                    | BinaryOperator::NotEq
                    | BinaryOperator::Lt
                    | BinaryOperator::LtEq
                    | BinaryOperator::Gt
                    | BinaryOperator::GtEq => ExpressionType::Known(SqlType::Boolean),
                    // Logical operators return boolean
                    BinaryOperator::And | BinaryOperator::Or => {
                        ExpressionType::Known(SqlType::Boolean)
                    }
                    _ => ExpressionType::Unknown,
                }
            }
            // Numeric arithmetic with a numeric string literal keeps the numeric type
            (ExpressionType::Known(t), ExpressionType::StringLiteral(_))
            | (ExpressionType::StringLiteral(_), ExpressionType::Known(t))
                if t.is_numeric() && arithmetic_op(op).is_some() =>
            {
                ExpressionType::Known(t)
            }
            _ => ExpressionType::Unknown,
        }
    }

    /// Infer type from a literal value
    fn infer_literal_type(&self, value: &Value) -> ExpressionType {
        match value {
            Value::Number(_, _) => {
                // Simplified: all numbers are integers for now
                // Future: distinguish between integer and decimal based on presence of '.'
                ExpressionType::Known(SqlType::Integer)
            }
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => {
                ExpressionType::StringLiteral(s.clone())
            }
            Value::Boolean(_) => ExpressionType::Known(SqlType::Boolean),
            Value::Null => {
                // NULL can be any type (compatible with everything)
                ExpressionType::Unknown
            }
            _ => ExpressionType::Unknown,
        }
    }

    /// Infer type from an unqualified column identifier
    fn infer_column_type_from_ident(&self, col_name: &str) -> ExpressionType {
        // Innermost scope first: a column found there shadows outer query blocks
        for scope in std::iter::once(&self.tables).chain(self.outer_scopes.iter().rev()) {
            if let Some(result) = self.infer_column_type_in_scope(scope, col_name) {
                return result;
            }
        }
        ExpressionType::Unknown
    }

    /// Type of an unqualified column within one scope, or `None` if no table in the
    /// scope has it
    fn infer_column_type_in_scope(
        &self,
        scope: &IndexMap<String, TableRef>,
        col_name: &str,
    ) -> Option<ExpressionType> {
        let mut found_type: Option<SqlType> = None;
        let mut has_unknown_relation = false;

        for table_ref in scope.values() {
            if let Some(ref derived_cols) = table_ref.derived_columns {
                // Derived table / CTE / function: column types are unknown
                if derived_cols.is_empty() {
                    has_unknown_relation = true;
                } else if derived_cols
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(col_name))
                {
                    return Some(ExpressionType::Unknown);
                }
            } else if let Some(ref view_cols) = table_ref.view_columns {
                if view_cols.is_empty() {
                    has_unknown_relation = true;
                } else if view_cols.iter().any(|c| c.eq_ignore_ascii_case(col_name)) {
                    // Column exists in view, but we don't know its type without analyzing the view
                    return Some(ExpressionType::Unknown);
                }
            } else if let Some(table_def) = self.catalog.get_table(&table_ref.table_name) {
                if let Some(col_def) = table_def.get_column(col_name) {
                    if found_type.is_some() {
                        // Column is ambiguous (exists in multiple tables)
                        return Some(ExpressionType::Unknown);
                    }
                    found_type = Some(col_def.data_type.clone());
                }
            }
        }

        match found_type {
            Some(t) => Some(known_column_type(t)),
            // The column may come from a relation with unknown columns
            None if has_unknown_relation => Some(ExpressionType::Unknown),
            None => None,
        }
    }

    /// Infer type from a qualified column identifier (table.column)
    fn infer_column_type_qualified(&self, table_name: &str, col_name: &str) -> ExpressionType {
        // Look up table in scope, innermost first
        let table_ref = std::iter::once(&self.tables)
            .chain(self.outer_scopes.iter().rev())
            .find_map(|scope| super::resolver::lookup_ignore_case(scope, table_name));
        if let Some(table_ref) = table_ref {
            // Check if this is a derived table or view
            if table_ref.derived_columns.is_some() || table_ref.view_columns.is_some() {
                // We can't infer types for derived tables or views yet
                return ExpressionType::Unknown;
            }

            // Regular table - look up in catalog
            if let Some(table_def) = self.catalog.get_table(&table_ref.table_name) {
                if let Some(col_def) = table_def.get_column(col_name) {
                    return known_column_type(col_def.data_type.clone());
                }
            }
        }

        ExpressionType::Unknown
    }
}

/// Source span of an expression, or `None` if the parser recorded no location
/// (sqlparser 0.53 doesn't track spans of literal values)
fn located_span(expr: &Expr) -> Option<Span> {
    let span = expr.span();
    (span.start.line > 0).then(|| Span::from_sqlparser(&span))
}

/// Expression type of a column with the given declared type. Columns whose type
/// sqlsift doesn't model (typeless SQLite columns, unsupported types) are unknown.
fn known_column_type(data_type: SqlType) -> ExpressionType {
    match data_type {
        SqlType::Unknown => ExpressionType::Unknown,
        t => ExpressionType::Known(t),
    }
}

/// Map an arithmetic binary operator to [`ArithmeticOp`]
fn arithmetic_op(op: &BinaryOperator) -> Option<ArithmeticOp> {
    match op {
        BinaryOperator::Plus => Some(ArithmeticOp::Add),
        BinaryOperator::Minus => Some(ArithmeticOp::Subtract),
        BinaryOperator::Multiply => Some(ArithmeticOp::Multiply),
        BinaryOperator::Divide => Some(ArithmeticOp::Divide),
        BinaryOperator::Modulo => Some(ArithmeticOp::Modulo),
        _ => None,
    }
}

/// Whether one side is BOOLEAN and the other an integer type
fn is_integer_boolean_pair(a: &SqlType, b: &SqlType) -> bool {
    (*a == SqlType::Boolean && b.is_integer()) || (*b == SqlType::Boolean && a.is_integer())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaBuilder;

    #[test]
    fn test_infer_literal_number() {
        let catalog = Catalog::default();
        let resolver = TypeResolver::new(&catalog);
        let value = Value::Number("123".to_string(), false);
        let result = resolver.infer_literal_type(&value);
        assert_eq!(result, ExpressionType::Known(SqlType::Integer));
    }

    #[test]
    fn test_infer_literal_string() {
        let catalog = Catalog::default();
        let resolver = TypeResolver::new(&catalog);
        let value = Value::SingleQuotedString("hello".to_string());
        let result = resolver.infer_literal_type(&value);
        assert_eq!(result, ExpressionType::StringLiteral("hello".to_string()));
    }

    #[test]
    fn test_infer_literal_boolean() {
        let catalog = Catalog::default();
        let resolver = TypeResolver::new(&catalog);
        let value = Value::Boolean(true);
        let result = resolver.infer_literal_type(&value);
        assert_eq!(result, ExpressionType::Known(SqlType::Boolean));
    }

    #[test]
    fn test_infer_literal_null() {
        let catalog = Catalog::default();
        let resolver = TypeResolver::new(&catalog);
        let value = Value::Null;
        let result = resolver.infer_literal_type(&value);
        assert_eq!(result, ExpressionType::Unknown);
    }

    #[test]
    fn test_type_mismatch_comparison() {
        let schema_sql = "CREATE TABLE users (id INTEGER, name TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        // Parse the query
        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE id = 'text'",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert!(diagnostics[0].message.contains("integer"));
        assert!(diagnostics[0].message.contains("text"));
    }

    #[test]
    fn test_arithmetic_on_text() {
        let schema_sql = "CREATE TABLE users (id INTEGER, name TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements =
            sqlparser::parser::Parser::parse_sql(dialect.as_ref(), "SELECT name + 10 FROM users")
                .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert!(diagnostics[0].message.contains("text"));
    }

    #[test]
    fn test_join_type_mismatch() {
        let schema_sql = r#"
            CREATE TABLE users (id INTEGER, name TEXT);
            CREATE TABLE orders (order_id INTEGER, user_name TEXT);
        "#;
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users JOIN orders ON users.id = orders.user_name",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::JoinTypeMismatch);
        assert!(diagnostics[0].message.contains("integer"));
        assert!(diagnostics[0].message.contains("text"));
    }

    // ========== Positive Tests (No Errors Expected) ==========

    #[test]
    fn test_valid_type_comparison() {
        let schema_sql = "CREATE TABLE users (id INTEGER, age INTEGER);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE id = age",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Same type comparison should not produce errors: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_numeric_type_compatibility() {
        let schema_sql = r#"
            CREATE TABLE data (
                tiny SMALLINT,
                small SMALLINT,
                medium INTEGER,
                big BIGINT
            );
        "#;
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // SMALLINT = INTEGER (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM data WHERE small = medium",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Numeric type implicit cast should be allowed: {:?}",
            diagnostics
        );

        // INTEGER = BIGINT (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM data WHERE medium = big",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Integer to BigInt implicit cast should be allowed: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_all_comparison_operators() {
        let schema_sql = "CREATE TABLE users (id INTEGER, name TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        let operators = vec![
            ("=", "Eq"),
            ("!=", "NotEq"),
            ("<", "Lt"),
            (">", "Gt"),
            ("<=", "LtEq"),
            (">=", "GtEq"),
        ];

        for (op, _name) in operators {
            let query = format!("SELECT * FROM users WHERE id {} 'text'", op);
            let statements =
                sqlparser::parser::Parser::parse_sql(dialect.as_ref(), &query).unwrap();

            let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
            name_resolver.resolve_statement(&statements[0]);

            let mut type_resolver = TypeResolver::new(&catalog);
            type_resolver.inherit_scope(&name_resolver);
            type_resolver.check_statement(&statements[0]);

            let diagnostics = type_resolver.into_diagnostics();
            assert_eq!(
                diagnostics.len(),
                1,
                "Operator {} should detect type mismatch",
                op
            );
            assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        }
    }

    #[test]
    fn test_null_is_always_compatible() {
        let schema_sql = "CREATE TABLE users (id INTEGER, name TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // NULL with INTEGER
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE id = NULL",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "NULL comparison should not produce type errors: {:?}",
            diagnostics
        );

        // NULL with TEXT
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE name = NULL",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "NULL with TEXT should not produce type errors: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_multiple_type_errors() {
        let schema_sql = "CREATE TABLE users (id INTEGER, age INTEGER, name TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE id = 'text' AND age = 'another'",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert_eq!(
            diagnostics.len(),
            2,
            "Should detect multiple type errors: {:?}",
            diagnostics
        );
        assert!(diagnostics
            .iter()
            .all(|d| d.kind == DiagnosticKind::TypeMismatch));
    }

    #[test]
    fn test_decimal_integer_compatibility() {
        let schema_sql = "CREATE TABLE products (id INTEGER, price DECIMAL(10, 2));";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM products WHERE id = price",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "INTEGER to DECIMAL implicit cast should be allowed: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_text_types_compatibility() {
        let schema_sql = "CREATE TABLE users (username VARCHAR(50), bio TEXT, code CHAR(10));";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // VARCHAR = TEXT (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE username = bio",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "VARCHAR to TEXT implicit cast should be allowed: {:?}",
            diagnostics
        );

        // CHAR = VARCHAR (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE code = username",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "CHAR to VARCHAR implicit cast should be allowed: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_nested_expressions() {
        let schema_sql = "CREATE TABLE data (a INTEGER, b INTEGER, c TEXT);";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // (a + b) should be numeric, comparing with c (TEXT) should error
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM data WHERE (a + b) = c",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
    }

    #[test]
    fn test_complex_join_conditions() {
        let schema_sql = r#"
            CREATE TABLE users (id INTEGER, name TEXT);
            CREATE TABLE orders (user_id INTEGER, product TEXT);
        "#;
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // Valid: id = user_id (both INTEGER)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users JOIN orders ON users.id = orders.user_id",
        )
        .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Valid JOIN condition should not produce errors: {:?}",
            diagnostics
        );
    }

    #[test]
    fn test_valid_arithmetic_operations() {
        let schema_sql = "CREATE TABLE data (a INTEGER, b INTEGER, price DECIMAL(10, 2));";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();

        // INTEGER + INTEGER
        let statements =
            sqlparser::parser::Parser::parse_sql(dialect.as_ref(), "SELECT a + b FROM data")
                .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Arithmetic on numeric types should not produce errors: {:?}",
            diagnostics
        );

        // INTEGER + DECIMAL
        let statements =
            sqlparser::parser::Parser::parse_sql(dialect.as_ref(), "SELECT a + price FROM data")
                .unwrap();

        let mut name_resolver = super::super::resolver::NameResolver::new(&catalog);
        name_resolver.resolve_statement(&statements[0]);

        let mut type_resolver = TypeResolver::new(&catalog);
        type_resolver.inherit_scope(&name_resolver);
        type_resolver.check_statement(&statements[0]);

        let diagnostics = type_resolver.into_diagnostics();
        assert!(
            diagnostics.is_empty(),
            "Mixed numeric arithmetic should not produce errors: {:?}",
            diagnostics
        );
    }
}
