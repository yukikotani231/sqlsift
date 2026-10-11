//! Type inference and type checks of the statement resolver
//!
//! Expression types are inferred from the [`Scope`](super::scope::Scope) the
//! resolver builds, so columns of CTEs, subqueries and views have the types of the
//! expressions that produce them. Anything that can't be inferred is
//! [`ExpressionType::Unknown`] and never reported.
//!
//! Checks:
//! - comparisons and arithmetic (E0003), JOIN conditions (E0007)
//! - `x IN (SELECT ...)` against the subquery's column
//! - INSERT VALUES / UPDATE SET / ON CONFLICT assignments against column types (E0003),
//!   and NULL assigned to NOT NULL columns (E0004); literals out of the column's range
//!   are reported in [`ranges`](super::ranges) (E0017)
//! - CASE branch consistency, set operation column counts and types

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    Insert, Parens, SetExpr, Spanned, Value, ValueWithSpan,
};

use crate::ast_ext::{column_ident, insert_table, name_last, ObjectNamePartExt};
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
#[cfg(test)]
use crate::schema::Catalog;
use crate::schema::{split_dotted_idents, TableDef};
use crate::types::{ArithmeticOp, SqlType, TypeCompatibility};

use super::resolver::{is_comparison_operator, Resolver};
use super::scope::{split_column_ref, Column, ColumnLookup, ColumnMatch, ExpressionType};
use crate::suggest::find_similar_name;

impl Resolver<'_> {
    // ---------------------------------------------------------------------
    // Checks
    // ---------------------------------------------------------------------

    /// Report a string literal that is not a value of the enum type on the other side.
    /// `column` names the enum-typed column (`table.column`), if it is one: inline
    /// `ENUM(...)` types have no name of their own. Returns true if a diagnostic was
    /// emitted.
    fn report_enum_literal(
        &mut self,
        left: &ExpressionType,
        right: &ExpressionType,
        column: Option<String>,
        span: Option<Span>,
    ) -> bool {
        let ((ExpressionType::Known(enum_type), ExpressionType::StringLiteral(literal))
        | (ExpressionType::StringLiteral(literal), ExpressionType::Known(enum_type))) =
            (left, right)
        else {
            return false;
        };
        // Named enum type (CREATE TYPE ... AS ENUM) or inline ENUM(...) column type
        let catalog = self.catalog;
        let (target, values) = match enum_type {
            SqlType::Custom(name) => match catalog.get_enum(name) {
                Some(enum_def) => (
                    format!("enum type '{}'", enum_def.name),
                    enum_def.values.as_slice(),
                ),
                None => return false,
            },
            SqlType::Enum(values) => match column {
                Some(column) => (format!("enum column '{column}'"), values.as_slice()),
                None => (
                    format!("enum type '{}'", enum_type.display_name_in(self.dialect)),
                    values.as_slice(),
                ),
            },
            _ => return false,
        };
        if values.is_empty() || values.iter().any(|v| v == literal) {
            return false;
        }
        let help = find_similar_name(values.iter().cloned(), literal).map_or_else(
            || {
                format!(
                    "Valid values: {}",
                    values
                        .iter()
                        .map(|v| format!("'{v}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            |s| format!("Did you mean '{s}'?"),
        );
        let mut diag = Diagnostic::error(
            DiagnosticKind::TypeMismatch,
            format!("Invalid value '{literal}' for {target}"),
        )
        .with_help(help);
        diag.span = span;
        self.diagnostics.push(diag);
        true
    }

    /// The display names of two types that can't be compared or assigned, or `None`
    /// if they are compatible (or either is unknown)
    fn type_conflict(
        &self,
        left: &ExpressionType,
        right: &ExpressionType,
    ) -> Option<(String, String)> {
        match (left, right) {
            (ExpressionType::Known(lt), ExpressionType::Known(rt)) => {
                if self.dialect.has_integer_booleans() && is_integer_boolean_pair(lt, rt) {
                    // MySQL BOOLEAN is TINYINT(1); SQLite stores booleans as integers
                    return None;
                }
                if self.dialect.coerces_strings() && is_coercible_pair(lt, rt) {
                    // Snowflake converts strings (and numbers to booleans) at run time
                    return None;
                }
                let compatible = lt.is_compatible_with(rt) != TypeCompatibility::ExplicitCast
                    || rt.is_compatible_with(lt) != TypeCompatibility::ExplicitCast;
                (!compatible).then(|| {
                    (
                        lt.display_name_in(self.dialect),
                        rt.display_name_in(self.dialect),
                    )
                })
            }
            (ExpressionType::Known(t), ExpressionType::StringLiteral(lit)) => {
                (!t.accepts_string_literal(lit)).then(|| {
                    (
                        t.display_name_in(self.dialect),
                        SqlType::Text.display_name_in(self.dialect),
                    )
                })
            }
            (ExpressionType::StringLiteral(lit), ExpressionType::Known(t)) => {
                (!t.accepts_string_literal(lit)).then(|| {
                    (
                        SqlType::Text.display_name_in(self.dialect),
                        t.display_name_in(self.dialect),
                    )
                })
            }
            _ => None,
        }
    }

    /// Check a comparison's operand types (E0003)
    fn check_comparison(&mut self, left: &Expr, right: &Expr) {
        let left_type = self.infer_expr_type(left);
        let right_type = self.infer_expr_type(right);
        // Literals carry no location: use the other operand's
        let span = located_span(left).or_else(|| located_span(right));
        let column = self.column_label(left).or_else(|| self.column_label(right));
        if self.report_enum_literal(&left_type, &right_type, column, span) {
            return;
        }
        if let Some((lt, rt)) = self.type_conflict(&left_type, &right_type) {
            let mut diag = Diagnostic::error(
                DiagnosticKind::TypeMismatch,
                format!("Type mismatch: cannot compare {lt} with {rt}"),
            )
            .with_help("Types are not implicitly compatible. Consider using explicit CAST.");
            diag.span = span;
            self.diagnostics.push(diag);
        }
    }

    /// Check a comparison in a JOIN ... ON condition (E0007)
    pub(super) fn check_join_comparison(&mut self, left: &Expr, right: &Expr) {
        let left_type = self.infer_expr_type(left);
        let right_type = self.infer_expr_type(right);
        let span = located_span(left).or_else(|| located_span(right));
        let column = self.column_label(left).or_else(|| self.column_label(right));
        if self.report_enum_literal(&left_type, &right_type, column, span) {
            return;
        }
        if let Some((lt, rt)) = self.type_conflict(&left_type, &right_type) {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticKind::JoinTypeMismatch,
                    format!("JOIN condition type mismatch: {lt} vs {rt}"),
                )
                .with_span(span.unwrap_or_else(|| Span::from_sqlparser(&left.span())))
                .with_help(
                    "JOIN condition should compare compatible types. Consider using explicit CAST.",
                ),
            );
        }
    }

    /// Check `expr IN (SELECT col ...)`: the value must be comparable with the column
    pub(super) fn check_in_subquery(&mut self, expr: &Expr, columns: Option<&[Column]>) {
        let Some([column]) = columns else {
            return;
        };
        let value_type = self.infer_expr_type(expr);
        if let Some((lt, rt)) = self.type_conflict(&value_type, &column.ty) {
            let mut diag = Diagnostic::error(
                DiagnosticKind::TypeMismatch,
                format!(
                    "Type mismatch: cannot compare {} with {} (column '{}' of the subquery)",
                    lt, rt, column.name
                ),
            )
            .with_help("Types are not implicitly compatible. Consider using explicit CAST.");
            diag.span = located_span(expr);
            self.diagnostics.push(diag);
        }
    }

    /// Check type compatibility in a binary operation. `left_type` is the type of
    /// `left` if the caller already inferred it. Returns the operation's result
    /// type if it was inferred along the way (arithmetic), so that a chain of
    /// operators (`a + b + c ...`) is type checked in linear time.
    pub(super) fn check_binary_op(
        &mut self,
        left: &Expr,
        op: &BinaryOperator,
        right: &Expr,
        left_type: Option<ExpressionType>,
    ) -> Option<ExpressionType> {
        if is_comparison_operator(op) {
            self.check_comparison(left, right);
            return None;
        }

        let arith_op = arithmetic_op(op)?;
        let left_type = left_type.unwrap_or_else(|| self.infer_expr_type(left));
        let right_type = self.infer_expr_type(right);
        self.check_arithmetic(left, arith_op, right, &left_type, &right_type);
        Some(binary_op_result_type(left_type, op, right_type))
    }

    /// Check the operand types of an arithmetic operation
    fn check_arithmetic(
        &mut self,
        left: &Expr,
        arith_op: ArithmeticOp,
        right: &Expr,
        left_type: &ExpressionType,
        right_type: &ExpressionType,
    ) {
        // Numeric arithmetic with a string literal: the literal must be a number
        let literal_operand = match (left_type, right_type) {
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
                            t.display_name_in(self.dialect)
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
        let (ExpressionType::Known(lt), ExpressionType::Known(rt)) = (left_type, right_type) else {
            return;
        };
        // A type whose operators sqlsift doesn't model may define this one
        // (MONEY * INTEGER, a domain over a number, ...)
        if !lt.has_known_arithmetic() || !rt.has_known_arithmetic() {
            return;
        }
        if SqlType::temporal_arithmetic_result(lt, arith_op, rt).is_some() {
            return;
        }
        // An interval or date/time operand with a numeric one is fine only in the
        // combinations allowed above; otherwise report each non-numeric side
        // (Snowflake converts a string operand to a number at run time)
        for (ty, expr) in [(lt, left), (rt, right)] {
            let numeric = ty.is_numeric() || (self.dialect.coerces_strings() && ty.is_string());
            if !numeric {
                self.diagnostics.push(
                    Diagnostic::error(
                        DiagnosticKind::TypeMismatch,
                        format!(
                            "Arithmetic operation requires numeric types, but got {}",
                            ty.display_name_in(self.dialect)
                        ),
                    )
                    .with_span(Span::from_sqlparser(&expr.span())),
                );
            }
        }
    }

    /// Report the first CASE branch whose type can't be reconciled with the others
    pub(super) fn check_case_branches(&mut self, branches: &[&Expr]) {
        let typed: Vec<(&Expr, ExpressionType)> = branches
            .iter()
            .map(|b| (*b, self.infer_expr_type(b)))
            .collect();
        let Some((_, reference)) = typed
            .iter()
            .find(|(_, t)| matches!(t, ExpressionType::Known(_)))
        else {
            return;
        };
        for (expr, ty) in &typed {
            if let Some((expected, actual)) = self.type_conflict(reference, ty) {
                let mut diag = Diagnostic::error(
                    DiagnosticKind::TypeMismatch,
                    format!("CASE branches have incompatible types: {expected} and {actual}"),
                )
                .with_help("All THEN/ELSE results of a CASE expression must have compatible types");
                diag.span =
                    located_span(expr).or_else(|| typed.iter().find_map(|(e, _)| located_span(e)));
                self.diagnostics.push(diag);
                return;
            }
        }
    }

    /// Check the branches of a set operation: same column count, compatible types
    pub(super) fn check_set_operation(
        &mut self,
        left: &[Column],
        right: &[Column],
        right_expr: &SetExpr,
    ) {
        if left.len() != right.len() {
            self.diagnostics.push(
                Diagnostic::error(
                    DiagnosticKind::TypeMismatch,
                    format!(
                        "Set operation column count mismatch: left has {}, right has {}",
                        left.len(),
                        right.len()
                    ),
                )
                .with_span(Span::from_sqlparser(&right_expr.span()))
                .with_help("UNION/INTERSECT/EXCEPT requires both sides to have the same number of columns."),
            );
            return;
        }

        for (idx, (l, r)) in left.iter().zip(right).enumerate() {
            if let Some((lt, rt)) = self.type_conflict(&l.ty, &r.ty) {
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
                    .with_span(Span::from_sqlparser(&right_expr.span()))
                    .with_help("Corresponding columns in UNION/INTERSECT/EXCEPT should be type-compatible."),
                );
            }
        }
    }

    /// Check INSERT ... VALUES rows against the target columns' types (E0003) and
    /// NOT NULL constraints (E0004)
    pub(super) fn check_insert_values(
        &mut self,
        insert: &Insert,
        table_def: &TableDef,
        rows: &[Parens<Vec<Expr>>],
    ) {
        // Without a column list, values are positional over all table columns
        let target_columns: Vec<&str> = if insert.columns.is_empty() {
            self.positional_columns(table_def)
        } else {
            insert
                .columns
                .iter()
                .map(|c| column_ident(c).value.as_str())
                .collect()
        };

        for row in rows {
            for (i, value_expr) in row.iter().enumerate().take(target_columns.len()) {
                let col_name = target_columns[i];
                let Some(col_def) = table_def.get_column(col_name) else {
                    continue; // Column not found - already reported
                };

                // MySQL/SQLite generate the key when NULL is inserted into an
                // integer primary key (AUTO_INCREMENT / rowid alias)
                let single_column_key = table_def
                    .primary_key
                    .as_ref()
                    .map_or(col_def.is_primary_key, |pk| {
                        pk.columns.len() == 1 && pk.columns[0].eq_ignore_ascii_case(&col_def.name)
                    });
                let generates_key = self.dialect.null_generates_integer_key()
                    && single_column_key
                    && col_def.data_type.is_integer();
                if !col_def.nullable
                    && !generates_key
                    && !table_def.before_insert_trigger
                    && matches!(
                        value_expr,
                        Expr::Value(ValueWithSpan {
                            value: Value::Null,
                            ..
                        })
                    )
                {
                    // NULL literals carry no source location: point at the
                    // target column, or the table name without a column list
                    let span = Span::from_sqlparser(
                        &insert
                            .columns
                            .get(i)
                            .map(|c| column_ident(c).span)
                            .or_else(|| insert_table(insert).and_then(name_last).map(|t| t.span))
                            .unwrap_or_else(|| value_expr.span()),
                    );
                    self.diagnostics.push(not_null_violation(col_name, span));
                    continue;
                }

                let value_type = self.infer_expr_type(value_expr);
                let column_type = ExpressionType::of_column(&col_def.data_type);
                let column_span = insert
                    .columns
                    .get(i)
                    .map(|c| Span::from_sqlparser(&column_ident(c).span));
                let column = format!("{}.{}", table_def.name, col_def.name);
                if self.report_enum_literal(&column_type, &value_type, Some(column), column_span) {
                    continue;
                }
                if let Some((expected, actual)) = self.type_conflict(&column_type, &value_type) {
                    let mut diag = assignment_mismatch(col_name, &expected, &actual);
                    // Literals carry no location: fall back to the target column
                    diag.span = located_span(value_expr).or(column_span);
                    self.diagnostics.push(diag);
                    continue;
                }
                let fallback = column_span.or_else(|| {
                    insert_table(insert)
                        .and_then(name_last)
                        .map(|t| Span::from_sqlparser(&t.span))
                });
                self.check_value_range(&table_def.name, col_def, value_expr, fallback);
            }
        }
    }

    /// Check `SET col = value` assignments against the target table's column types
    pub(super) fn check_assignments(
        &mut self,
        table_def: &TableDef,
        assignments: &[sqlparser::ast::Assignment],
    ) {
        for assignment in assignments {
            let AssignmentTarget::ColumnName(name) = &assignment.target else {
                continue; // Tuple assignments aren't checked
            };
            let Some(target) = name.0.last() else {
                continue;
            };
            let Some(col_def) = table_def.get_column(&target.ident().value) else {
                continue; // Column not found - already reported
            };
            let target_span = Span::from_sqlparser(&target.ident().span);

            if !col_def.nullable
                && !table_def.before_update_trigger
                && matches!(
                    &assignment.value,
                    Expr::Value(ValueWithSpan {
                        value: Value::Null,
                        ..
                    })
                )
            {
                // NULL literals carry no source location: point at the target column
                self.diagnostics
                    .push(not_null_violation(&target.ident().value, target_span));
                continue;
            }

            let value_type = self.infer_expr_type(&assignment.value);
            let column_type = ExpressionType::of_column(&col_def.data_type);
            let column = format!("{}.{}", table_def.name, col_def.name);
            if self.report_enum_literal(&column_type, &value_type, Some(column), Some(target_span))
            {
                continue;
            }
            if let Some((expected, actual)) = self.type_conflict(&column_type, &value_type) {
                let mut diag = assignment_mismatch(&target.ident().value, &expected, &actual);
                // Literals carry no location: fall back to the target column
                diag.span = located_span(&assignment.value).or(Some(target_span));
                self.diagnostics.push(diag);
                continue;
            }
            self.check_value_range(
                &table_def.name,
                col_def,
                &assignment.value,
                Some(target_span),
            );
        }
    }

    // ---------------------------------------------------------------------
    // Inference
    // ---------------------------------------------------------------------

    /// Infer the type of an expression in the current scope
    pub(super) fn infer_expr_type(&self, expr: &Expr) -> ExpressionType {
        match expr {
            Expr::Value(value) => infer_literal_type(&value.value),
            Expr::Identifier(ident) => match self.scope.column(&ident.value, self.dialect) {
                ColumnLookup::Found(ty) => ty.clone(),
                _ => ExpressionType::Unknown,
            },
            Expr::CompoundIdentifier(parts) if self.dialect.struct_field_paths() => {
                self.infer_bigquery_path(parts)
            }
            Expr::CompoundIdentifier(parts) => {
                // [[database.]schema.]table.column
                let parts = split_dotted_idents(parts);
                match split_column_ref(&parts) {
                    Some((schema, table, column)) => {
                        self.infer_qualified_column(schema, table, column)
                    }
                    None => ExpressionType::Unknown,
                }
            }
            Expr::Nested(inner) => self.infer_expr_type(inner),
            Expr::BinaryOp { .. } => self.infer_binary_chain_type(expr),
            Expr::Cast { data_type, .. } => {
                ExpressionType::of_column(&SqlType::from_ast_in(data_type, self.dialect))
            }
            Expr::Function(func) => self.infer_function_return_type(func),
            Expr::Interval(_) => ExpressionType::Known(SqlType::Interval),
            Expr::Case {
                conditions,
                else_result,
                ..
            } => {
                let branches: Vec<&Expr> = conditions
                    .iter()
                    .map(|c| &c.result)
                    .chain(else_result.as_deref())
                    .collect();
                self.infer_case_type(&branches)
            }
            // Typed literals such as DATE '2024-01-01'
            Expr::TypedString(typed) => {
                ExpressionType::of_column(&SqlType::from_ast_in(&typed.data_type, self.dialect))
            }
            // Scalar subquery: the type of its single column
            Expr::Subquery(query) => match self
                .subquery_columns
                .get(&(query.as_ref() as *const _ as usize))
            {
                Some(Some(columns)) if columns.len() == 1 => columns[0].ty.clone(),
                _ => ExpressionType::Unknown,
            },
            _ => ExpressionType::Unknown,
        }
    }

    /// Type of `table.column`
    /// `table.column` for an expression that is a column reference
    fn column_label(&self, expr: &Expr) -> Option<String> {
        let (relation, column) = match expr {
            Expr::Identifier(ident) => (
                self.scope.column_relation(&ident.value, self.dialect)?,
                ident,
            ),
            Expr::CompoundIdentifier(parts) => {
                let parts = split_dotted_idents(parts);
                let (schema, table, column) = split_column_ref(&parts)?;
                let relation = self.scope.qualified_relation(self.catalog, schema, table)?;
                return Some(format!("{}.{}", relation.name, column.value));
            }
            Expr::Nested(inner) => return self.column_label(inner),
            _ => return None,
        };
        Some(format!("{}.{}", relation.name, column.value))
    }

    pub(super) fn infer_qualified_column(
        &self,
        schema: Option<&sqlparser::ast::Ident>,
        table: &sqlparser::ast::Ident,
        column: &sqlparser::ast::Ident,
    ) -> ExpressionType {
        match self
            .scope
            .qualified_relation(self.catalog, schema, table)
            .map(|r| r.column(&column.value, self.dialect))
        {
            Some(ColumnMatch::Yes(ty)) => ty.clone(),
            _ => ExpressionType::Unknown,
        }
    }

    /// Result type of a CASE expression: the first branch with a known type, or text
    /// when every branch is a string literal (as in PostgreSQL)
    fn infer_case_type(&self, branches: &[&Expr]) -> ExpressionType {
        let mut types: Vec<ExpressionType> =
            branches.iter().map(|b| self.infer_expr_type(b)).collect();
        if let Some(i) = types
            .iter()
            .position(|t| matches!(t, ExpressionType::Known(_)))
        {
            return types.swap_remove(i);
        }
        if !types.is_empty()
            && types
                .iter()
                .all(|t| matches!(t, ExpressionType::StringLiteral(_)))
        {
            return ExpressionType::Known(SqlType::Text);
        }
        ExpressionType::Unknown
    }

    /// Infer the return type of a SQL function
    fn infer_function_return_type(&self, func: &sqlparser::ast::Function) -> ExpressionType {
        let func_name = func.name.to_string().to_uppercase();
        // Strip schema prefix (e.g., "PG_CATALOG.COUNT" → "COUNT")
        let name = func_name.rsplit('.').next().unwrap_or(&func_name);
        if self.dialect == SqlDialect::BigQuery {
            if let Some(ty) = self.bigquery_function_type(name, func) {
                return ty;
            }
        }

        if self.dialect == SqlDialect::Snowflake {
            if let Some(ty) = self.snowflake_function_type(name, func) {
                return ty;
            }
        }

        match name {
            // Aggregate functions returning INTEGER/BIGINT
            "COUNT" => ExpressionType::Known(SqlType::BigInt),

            // SUM: integer arguments → BIGINT, decimal / floating point arguments keep their type
            "SUM" => match self.infer_first_arg_type(func) {
                ExpressionType::Known(
                    t @ (SqlType::Decimal { .. } | SqlType::Real | SqlType::DoublePrecision),
                ) => ExpressionType::Known(t),
                ExpressionType::Known(t) if t.is_numeric() => {
                    ExpressionType::Known(SqlType::BigInt)
                }
                _ => ExpressionType::Unknown,
            },
            // AVG of numbers is NUMERIC (or floating point), AVG of INTERVAL an
            // interval; other argument types (MONEY, ...) are left unknown
            "AVG" => match self.infer_first_arg_type(func) {
                ExpressionType::Known(t @ (SqlType::Real | SqlType::DoublePrecision)) => {
                    ExpressionType::Known(t)
                }
                ExpressionType::Known(t) if t.is_numeric() => {
                    ExpressionType::Known(SqlType::Decimal {
                        precision: None,
                        scale: None,
                    })
                }
                ExpressionType::Known(SqlType::Interval) => {
                    ExpressionType::Known(SqlType::Interval)
                }
                _ => ExpressionType::Unknown,
            },

            // Functions returning the type of their first argument
            "MIN" | "MAX" | "COALESCE" | "NULLIF" | "IFNULL" | "GREATEST" | "LEAST" => {
                self.infer_first_arg_type(func)
            }
            "ABS" | "CEIL" | "CEILING" | "FLOOR" | "ROUND" | "TRUNC" | "TRUNCATE" | "SIGN"
            | "MOD" => match self.infer_first_arg_type(func) {
                known @ ExpressionType::Known(_) => known,
                _ => ExpressionType::Unknown,
            },

            // Boolean-returning functions
            "EXISTS" | "BOOL_AND" | "BOOL_OR" | "EVERY" => ExpressionType::Known(SqlType::Boolean),

            // String functions
            "CONCAT" | "UPPER" | "LOWER" | "TRIM" | "LTRIM" | "RTRIM" | "REPLACE" | "SUBSTRING"
            | "SUBSTR" | "LEFT" | "RIGHT" | "LPAD" | "RPAD" | "REPEAT" | "REVERSE" | "INITCAP"
            | "MD5" => ExpressionType::Known(SqlType::Text),

            // String → Integer functions
            "LENGTH" | "CHAR_LENGTH" | "CHARACTER_LENGTH" | "BIT_LENGTH" | "OCTET_LENGTH"
            | "POSITION" | "STRPOS" => ExpressionType::Known(SqlType::Integer),

            // Floating point functions
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

            _ => ExpressionType::Unknown,
        }
    }

    /// Return type of a Snowflake function, or `None` to use the generic rules.
    /// Functions returning semi-structured values (PARSE_JSON, OBJECT_CONSTRUCT,
    /// ARRAY_AGG, GET, ...) are left unknown.
    fn snowflake_function_type(
        &self,
        name: &str,
        func: &sqlparser::ast::Function,
    ) -> Option<ExpressionType> {
        let args = unnamed_args(func);
        let known = |t: SqlType| Some(ExpressionType::Known(t));
        let timestamp = |with_timezone| SqlType::Timestamp {
            precision: None,
            with_timezone,
        };
        let decimal = SqlType::Decimal {
            precision: None,
            scale: None,
        };
        // The type of a date/time argument (DATEADD on a DATE is a DATE, ...)
        let temporal_arg = |i: usize| match args.get(i).map(|e| self.infer_expr_type(e)) {
            Some(ExpressionType::Known(
                t @ (SqlType::Date | SqlType::Time { .. } | SqlType::Timestamp { .. }),
            )) => ExpressionType::Known(t),
            _ => ExpressionType::Unknown,
        };
        let numeric_arg = |i: usize| match args.get(i).map(|e| self.infer_expr_type(e)) {
            Some(ExpressionType::Known(t)) if t.is_numeric() => ExpressionType::Known(t),
            _ => ExpressionType::Unknown,
        };
        let branches = |indices: &mut dyn Iterator<Item = usize>| {
            let exprs: Vec<&Expr> = indices.filter_map(|i| args.get(i).copied()).collect();
            self.infer_case_type(&exprs)
        };
        match name {
            // Conditional expressions: the type of their result branches
            "IFF" => Some(branches(&mut (1..=2))),
            "NVL2" => Some(branches(&mut (1..=2))),
            "NVL" | "ANY_VALUE" => Some(self.infer_first_arg_type(func)),
            // DECODE(expr, search1, result1, ..., default)
            "DECODE" => {
                let results = (2..args.len()).step_by(2);
                let default = (args.len() > 1 && args.len() % 2 == 0).then(|| args.len() - 1);
                Some(branches(&mut results.chain(default)))
            }
            "ZEROIFNULL" | "NULLIFZERO" => Some(numeric_arg(0)),
            "DIV0" | "DIV0NULL" | "MEDIAN" => known(decimal),

            // Conversion functions (TRY_CAST and `::` are casts)
            "TO_NUMBER" | "TO_DECIMAL" | "TO_NUMERIC" | "TRY_TO_NUMBER" | "TRY_TO_DECIMAL"
            | "TRY_TO_NUMERIC" => known(decimal),
            "TO_DOUBLE" | "TRY_TO_DOUBLE" => known(SqlType::DoublePrecision),
            "TO_BOOLEAN" | "TRY_TO_BOOLEAN" => known(SqlType::Boolean),
            "TO_DATE" | "TRY_TO_DATE" | "DATE" | "DATE_FROM_PARTS" | "LAST_DAY" | "NEXT_DAY"
            | "PREVIOUS_DAY" => known(SqlType::Date),
            "TO_TIME" | "TRY_TO_TIME" | "TIME" | "TIME_FROM_PARTS" => known(SqlType::Time {
                precision: None,
                with_timezone: false,
            }),
            "TO_TIMESTAMP"
            | "TO_TIMESTAMP_NTZ"
            | "TRY_TO_TIMESTAMP"
            | "TRY_TO_TIMESTAMP_NTZ"
            | "TIMESTAMP_FROM_PARTS"
            | "TIMESTAMP_NTZ_FROM_PARTS"
            | "SYSDATE"
            | "LOCALTIMESTAMP" => known(timestamp(false)),
            "TO_TIMESTAMP_LTZ"
            | "TO_TIMESTAMP_TZ"
            | "TRY_TO_TIMESTAMP_LTZ"
            | "TRY_TO_TIMESTAMP_TZ"
            | "TIMESTAMP_LTZ_FROM_PARTS"
            | "TIMESTAMP_TZ_FROM_PARTS"
            | "CONVERT_TIMEZONE"
            | "GETDATE" => known(timestamp(true)),
            "TO_VARCHAR" | "TO_CHAR" | "TO_JSON" | "TYPEOF" => known(SqlType::Text),

            // Date/time arithmetic: DATEADD(part, n, date) keeps the date's type,
            // DATE_TRUNC(part, date) too
            "DATEADD" | "TIMEADD" | "TIMESTAMPADD" => Some(temporal_arg(2)),
            "DATE_TRUNC" => Some(temporal_arg(1)),
            "DATEDIFF" | "TIMEDIFF" | "TIMESTAMPDIFF" => known(SqlType::BigInt),
            "DATE_PART" | "YEAR" | "MONTH" | "DAY" | "DAYOFMONTH" | "DAYOFWEEK"
            | "DAYOFWEEKISO" | "DAYOFYEAR" | "WEEK" | "WEEKOFYEAR" | "WEEKISO" | "YEAROFWEEK"
            | "YEAROFWEEKISO" | "QUARTER" | "HOUR" | "MINUTE" | "SECOND" => known(SqlType::Integer),

            // Strings
            "LISTAGG" | "SPLIT_PART" | "STRTOK" | "REGEXP_SUBSTR" | "REGEXP_REPLACE"
            | "CONCAT_WS" | "ARRAY_TO_STRING" | "UUID_STRING" | "MD5_HEX" | "SHA1" | "SHA1_HEX"
            | "SHA2" | "SHA2_HEX" | "BASE64_ENCODE" | "HEX_ENCODE" | "CHR" | "TRANSLATE"
            | "SOUNDEX" => known(SqlType::Text),
            "LEN" | "CHARINDEX" | "REGEXP_COUNT" | "REGEXP_INSTR" | "EDITDISTANCE"
            | "ARRAY_SIZE" | "ASCII" => known(SqlType::Integer),
            "REGEXP_LIKE" | "RLIKE" | "CONTAINS" | "STARTSWITH" | "ENDSWITH" | "EQUAL_NULL"
            | "IS_NULL_VALUE" | "ARRAY_CONTAINS" | "BOOLAND" | "BOOLOR" | "BOOLXOR" => {
                known(SqlType::Boolean)
            }

            // Aggregates and sequences
            "HASH"
            | "COUNT_IF"
            | "APPROX_COUNT_DISTINCT"
            | "HLL"
            | "SEQ1"
            | "SEQ2"
            | "SEQ4"
            | "SEQ8"
            | "ROW_NUMBER"
            | "RANK"
            | "DENSE_RANK" => known(SqlType::BigInt),
            "BOOLAND_AGG" | "BOOLOR_AGG" | "BOOLXOR_AGG" => known(SqlType::Boolean),
            _ => None,
        }
    }

    /// Infer the type of the first argument to a function
    fn infer_first_arg_type(&self, func: &sqlparser::ast::Function) -> ExpressionType {
        if let FunctionArguments::List(arg_list) = &func.args {
            if let Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))) = arg_list.args.first() {
                return self.infer_expr_type(expr);
            }
        }
        ExpressionType::Unknown
    }

    /// Infer the result type of a binary operation. A chain of left-associative
    /// operators (`a + b + c ...`) nests as deep as it is long, so its left spine
    /// is walked in a loop rather than recursively.
    fn infer_binary_chain_type(&self, expr: &Expr) -> ExpressionType {
        let mut chain = Vec::new();
        let mut leftmost = expr;
        while let Expr::BinaryOp { left, op, right } = leftmost {
            chain.push((op, &**right));
            leftmost = left;
        }
        let mut ty = self.infer_expr_type(leftmost);
        for (op, right) in chain.into_iter().rev() {
            ty = binary_op_result_type(ty, op, self.infer_expr_type(right));
        }
        ty
    }
}

/// A function's positional (unnamed) argument expressions
fn unnamed_args(func: &sqlparser::ast::Function) -> Vec<&Expr> {
    let FunctionArguments::List(list) = &func.args else {
        return Vec::new();
    };
    list.args
        .iter()
        .filter_map(|arg| match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => Some(e),
            _ => None,
        })
        .collect()
}

/// Result type of a binary operation on operands of the given types
fn binary_op_result_type(
    left: ExpressionType,
    op: &BinaryOperator,
    right: ExpressionType,
) -> ExpressionType {
    match (left, right) {
        // Comparisons and logical operators over known operands are boolean
        (ExpressionType::Known(_), ExpressionType::Known(_))
            if is_comparison_operator(op)
                || matches!(op, BinaryOperator::And | BinaryOperator::Or) =>
        {
            ExpressionType::Known(SqlType::Boolean)
        }
        (ExpressionType::Known(lt), ExpressionType::Known(rt)) => {
            let Some(arith_op) = arithmetic_op(op) else {
                return ExpressionType::Unknown;
            };
            if lt.is_numeric() && rt.is_numeric() {
                // Simplified promotion: the left operand's type
                ExpressionType::Known(lt)
            } else {
                SqlType::temporal_arithmetic_result(&lt, arith_op, &rt)
                    .map_or(ExpressionType::Unknown, ExpressionType::Known)
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
fn infer_literal_type(value: &Value) -> ExpressionType {
    match value {
        // Simplified: all numbers are integers (compatible with every numeric type)
        Value::Number(_, _) => ExpressionType::Known(SqlType::Integer),
        Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => {
            ExpressionType::StringLiteral(s.clone())
        }
        Value::Boolean(_) => ExpressionType::Known(SqlType::Boolean),
        // NULL can be any type (compatible with everything)
        _ => ExpressionType::Unknown,
    }
}

/// E0004 diagnostic for NULL assigned to a NOT NULL column
fn not_null_violation(column: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        DiagnosticKind::PotentialNullViolation,
        format!(
            "Potential NOT NULL violation: column '{column}' cannot be assigned NULL"
        ),
    )
    .with_span(span)
    .with_help(
        "This column is defined as NOT NULL. Provide a non-NULL value or change the schema constraint.",
    )
}

/// E0003 diagnostic for a value whose type doesn't fit the target column
fn assignment_mismatch(column: &str, expected: &str, actual: &str) -> Diagnostic {
    Diagnostic::error(
        DiagnosticKind::TypeMismatch,
        format!("Type mismatch: column '{column}' expects {expected}, but got {actual}"),
    )
    .with_help("Value type is not compatible with the column type. Consider using explicit CAST.")
}

/// Source span of an expression, or `None` if the parser recorded no location.
/// A literal value counts as unlocated, so that its diagnostics point at the
/// column it is compared with or assigned to (as before sqlparser tracked the
/// spans of literals).
pub(super) fn located_span(expr: &Expr) -> Option<Span> {
    if matches!(expr, Expr::Value(_)) {
        return None;
    }
    let span = expr.span();
    (span.start.line > 0).then(|| Span::from_sqlparser(&span))
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

/// Whether Snowflake implicitly converts between the two types: a string and a
/// number, date/time or boolean, or a number and a boolean
fn is_coercible_pair(a: &SqlType, b: &SqlType) -> bool {
    let scalar = |t: &SqlType| {
        t.is_numeric()
            || *t == SqlType::Boolean
            || matches!(
                t,
                SqlType::Date | SqlType::Time { .. } | SqlType::Timestamp { .. }
            )
    };
    let numeric_boolean = |x: &SqlType, y: &SqlType| x.is_numeric() && *y == SqlType::Boolean;
    (a.is_string() && scalar(b))
        || (b.is_string() && scalar(a))
        || numeric_boolean(a, b)
        || numeric_boolean(b, a)
}

/// Whether one side is BOOLEAN and the other an integer type
fn is_integer_boolean_pair(a: &SqlType, b: &SqlType) -> bool {
    (*a == SqlType::Boolean && b.is_integer()) || (*b == SqlType::Boolean && a.is_integer())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaBuilder;

    /// Resolve and type check one statement
    fn check(catalog: &Catalog, stmt: &sqlparser::ast::Statement) -> Vec<Diagnostic> {
        let mut resolver = Resolver::new(catalog, SqlDialect::PostgreSQL);
        resolver.statement(stmt);
        resolver.into_diagnostics()
    }

    #[test]
    fn test_infer_literal_number() {
        let value = Value::Number("123".to_string(), false);
        let result = infer_literal_type(&value);
        assert_eq!(result, ExpressionType::Known(SqlType::Integer));
    }

    #[test]
    fn test_infer_literal_string() {
        let value = Value::SingleQuotedString("hello".to_string());
        let result = infer_literal_type(&value);
        assert_eq!(result, ExpressionType::StringLiteral("hello".to_string()));
    }

    #[test]
    fn test_infer_literal_boolean() {
        let value = Value::Boolean(true);
        let result = infer_literal_type(&value);
        assert_eq!(result, ExpressionType::Known(SqlType::Boolean));
    }

    #[test]
    fn test_infer_literal_null() {
        let value = Value::Null;
        let result = infer_literal_type(&value);
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

        let diagnostics = check(&catalog, &statements[0]);
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

        let diagnostics = check(&catalog, &statements[0]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
        assert!(diagnostics[0].message.contains("text"));
    }

    #[test]
    fn test_join_type_mismatch() {
        let schema_sql = r"
            CREATE TABLE users (id INTEGER, name TEXT);
            CREATE TABLE orders (order_id INTEGER, user_name TEXT);
        ";
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();

        let dialect = crate::dialect::SqlDialect::PostgreSQL.parser_dialect();
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users JOIN orders ON users.id = orders.user_name",
        )
        .unwrap();

        let diagnostics = check(&catalog, &statements[0]);
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Same type comparison should not produce errors: {diagnostics:?}"
        );
    }

    #[test]
    fn test_numeric_type_compatibility() {
        let schema_sql = r"
            CREATE TABLE data (
                tiny SMALLINT,
                small SMALLINT,
                medium INTEGER,
                big BIGINT
            );
        ";
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Numeric type implicit cast should be allowed: {diagnostics:?}"
        );

        // INTEGER = BIGINT (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM data WHERE medium = big",
        )
        .unwrap();

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Integer to BigInt implicit cast should be allowed: {diagnostics:?}"
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
            let query = format!("SELECT * FROM users WHERE id {op} 'text'");
            let statements =
                sqlparser::parser::Parser::parse_sql(dialect.as_ref(), &query).unwrap();

            let diagnostics = check(&catalog, &statements[0]);
            assert_eq!(
                diagnostics.len(),
                1,
                "Operator {op} should detect type mismatch"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.kind == DiagnosticKind::NullComparison),
            "NULL comparison should not produce type errors: {diagnostics:?}"
        );

        // NULL with TEXT
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE name = NULL",
        )
        .unwrap();

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.kind == DiagnosticKind::NullComparison),
            "NULL with TEXT should not produce type errors: {diagnostics:?}"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert_eq!(
            diagnostics.len(),
            2,
            "Should detect multiple type errors: {diagnostics:?}"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "INTEGER to DECIMAL implicit cast should be allowed: {diagnostics:?}"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "VARCHAR to TEXT implicit cast should be allowed: {diagnostics:?}"
        );

        // CHAR = VARCHAR (implicit cast allowed)
        let statements = sqlparser::parser::Parser::parse_sql(
            dialect.as_ref(),
            "SELECT * FROM users WHERE code = username",
        )
        .unwrap();

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "CHAR to VARCHAR implicit cast should be allowed: {diagnostics:?}"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::TypeMismatch);
    }

    #[test]
    fn test_complex_join_conditions() {
        let schema_sql = r"
            CREATE TABLE users (id INTEGER, name TEXT);
            CREATE TABLE orders (user_id INTEGER, product TEXT);
        ";
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Valid JOIN condition should not produce errors: {diagnostics:?}"
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

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Arithmetic on numeric types should not produce errors: {diagnostics:?}"
        );

        // INTEGER + DECIMAL
        let statements =
            sqlparser::parser::Parser::parse_sql(dialect.as_ref(), "SELECT a + price FROM data")
                .unwrap();

        let diagnostics = check(&catalog, &statements[0]);
        assert!(
            diagnostics.is_empty(),
            "Mixed numeric arithmetic should not produce errors: {diagnostics:?}"
        );
    }
}
