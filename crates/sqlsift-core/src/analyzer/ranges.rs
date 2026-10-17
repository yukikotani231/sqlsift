//! - E0017: a literal assigned to a column that can't hold it
//! - E0037: a literal that violates a CHECK constraint in INSERT VALUES or UPDATE SET
//!
//! `INSERT ... VALUES` and `UPDATE ... SET` with a string longer than a
//! `VARCHAR(n)` / `CHAR(n)` column, an integer outside the range of an integer
//! column, or a number with more integer digits than a `DECIMAL(p, s)` column
//! allows. Only literals are checked, and only for the dialects that reject the
//! value instead of truncating it (PostgreSQL, Redshift, MySQL in its default
//! strict mode). Comparisons such as `WHERE age = 99999` are valid and not checked.

use std::collections::{HashMap, HashSet};

use sqlparser::ast::{
    Assignment, AssignmentTarget, BinaryOperator, Expr, FunctionArg, FunctionArgExpr,
    FunctionArguments, Spanned, UnaryOperator, Value, ValueWithSpan,
};
use sqlparser::parser::Parser;

use crate::ast_ext::ObjectNamePartExt;
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::{ColumnDef, QualifiedName, TableDef};
use crate::types::SqlType;

use super::resolver::Resolver;

impl Resolver<'_> {
    /// Report `value` if it is a literal that `column` of `table` can't hold.
    /// `fallback` locates the diagnostic when the literal has no location.
    pub(super) fn check_value_range(
        &mut self,
        table: &QualifiedName,
        column: &ColumnDef,
        value: &Expr,
        fallback: Option<Span>,
    ) {
        if !self.dialect.rejects_out_of_range_values() {
            return;
        }
        let Some(problem) = out_of_range(self.dialect, &column.data_type, value) else {
            return;
        };
        let type_name = column.data_type.display_name();
        let target = format!("{table}.{}", column.name);
        let (message, help) = match problem {
            Problem::TooLong(length) => (
                format!(
                    "Value is too long for column '{target}' of type {type_name}: \
                     {length} characters"
                ),
                "Shorten the value or widen the column",
            ),
            Problem::OutOfRange(literal) => (
                format!(
                    "Value {literal} is out of range for column '{target}' of type {type_name}"
                ),
                "Use a value the column type can hold or a wider column type",
            ),
        };
        let span = super::type_check::located_span(value).or(fallback);
        let mut diag = Diagnostic::error(DiagnosticKind::ValueOutOfRange, message).with_help(help);
        diag.span = span;
        self.diagnostics.push(diag);
    }

    /// Check whether values in an INSERT VALUES row violate any CHECK constraints on `table_def`.
    pub(super) fn check_insert_check_constraints(
        &mut self,
        table_def: &TableDef,
        row: &[Expr],
        target_columns: &[&str],
        fallback: Option<Span>,
    ) {
        if !self.dialect.enforces_check_constraints() || table_def.check_constraints.is_empty() {
            return;
        }

        let mut assigned_columns = HashMap::new();
        for (i, value_expr) in row.iter().enumerate().take(target_columns.len()) {
            let col_name = target_columns[i].to_ascii_lowercase();
            let val_span = value_expr.span();
            let span = super::type_check::located_span(value_expr)
                .or_else(|| (val_span.start.line > 0).then(|| Span::from_sqlparser(&val_span)))
                .or(fallback);
            assigned_columns.insert(col_name, (value_expr, span));
        }

        self.evaluate_check_constraints(table_def, &assigned_columns, fallback);
    }

    /// Check whether assignments in an UPDATE SET violate any CHECK constraints on `table_def`.
    pub(super) fn check_update_check_constraints(
        &mut self,
        table_def: &TableDef,
        assignments: &[Assignment],
    ) {
        if !self.dialect.enforces_check_constraints() || table_def.check_constraints.is_empty() {
            return;
        }

        let mut assigned_columns = HashMap::new();
        for assignment in assignments {
            let AssignmentTarget::ColumnName(name) = &assignment.target else {
                continue;
            };
            let Some(target) = name.0.last() else {
                continue;
            };
            let col_name = target.ident().value.to_ascii_lowercase();
            let target_span = Span::from_sqlparser(&target.ident().span);
            let val_span = assignment.value.span();
            let span = super::type_check::located_span(&assignment.value)
                .or_else(|| (val_span.start.line > 0).then(|| Span::from_sqlparser(&val_span)))
                .or(Some(target_span));
            assigned_columns.insert(col_name, (&assignment.value, span));
        }

        self.evaluate_check_constraints(table_def, &assigned_columns, None);
    }

    fn evaluate_check_constraints(
        &mut self,
        table_def: &TableDef,
        assigned_columns: &HashMap<String, (&Expr, Option<Span>)>,
        fallback: Option<Span>,
    ) {
        'check_loop: for check in &table_def.check_constraints {
            // Skip NOT ENFORCED constraints
            if !check.enforced {
                continue;
            }

            let Ok(expr) = Parser::new(self.dialect.parser_dialect().as_ref())
                .try_with_sql(&check.expression)
                .and_then(|mut p| p.parse_expr())
            else {
                continue;
            };

            let mut referenced_cols = HashSet::new();
            collect_referenced_columns(&expr, &mut referenced_cols);

            if referenced_cols.is_empty() {
                continue;
            }

            // Only evaluate plain integer, numeric, and text columns without an explicit collation
            for col_name in &referenced_cols {
                let Some(col_def) = table_def.get_column(col_name) else {
                    continue 'check_loop;
                };
                if col_def.collated || !is_plain_integer_numeric_or_text(&col_def.data_type) {
                    continue 'check_loop;
                }
            }

            // Only evaluate when every column the CHECK expression reads is assigned
            if !referenced_cols
                .iter()
                .all(|c| assigned_columns.contains_key(c))
            {
                continue;
            }

            let mut env = HashMap::new();
            for col_name in &referenced_cols {
                let (val_expr, _) = assigned_columns[col_name];
                let Some(col_def) = table_def.get_column(col_name) else {
                    continue 'check_loop;
                };
                let const_val =
                    convert_literal_to_column_type(val_expr, &col_def.data_type, self.dialect);
                env.insert(col_name.clone(), const_val);
            }

            if eval_expr(&expr, &env, self.dialect) == EvalValue::Bool(false) {
                let constraint_clause = if let Some(ref name) = check.name {
                    format!("check constraint '{name}'")
                } else {
                    format!("check constraint ({})", check.expression)
                };
                let message = format!(
                    "Value violates {constraint_clause} for table '{}'",
                    table_def.name
                );
                let help = if let Some(ref name) = check.name {
                    format!(
                        "Ensure assigned values satisfy '{name}' ({})",
                        check.expression
                    )
                } else {
                    format!("Ensure assigned values satisfy '{}'", check.expression)
                };

                let mut span = None;
                for col_name in &referenced_cols {
                    if let Some((_, col_span)) = assigned_columns.get(col_name) {
                        if col_span.is_some() {
                            span = *col_span;
                            break;
                        }
                    }
                }
                let span = span.or(fallback);

                let mut diag = Diagnostic::error(DiagnosticKind::CheckConstraintViolation, message)
                    .with_help(help);
                diag.span = span;
                self.diagnostics.push(diag);
            }
        }
    }
}

enum Problem {
    /// A string with this many characters (trailing spaces not counted)
    TooLong(usize),
    /// A number, as written
    OutOfRange(String),
}

fn out_of_range(dialect: SqlDialect, column_type: &SqlType, value: &Expr) -> Option<Problem> {
    match (column_type, value) {
        (
            SqlType::Char {
                length: Some(limit),
            }
            | SqlType::Varchar {
                length: Some(limit),
            },
            Expr::Value(ValueWithSpan {
                value: Value::SingleQuotedString(s),
                ..
            }),
        ) => {
            // Excess trailing spaces are truncated silently
            let length = s.trim_end_matches(' ').chars().count();
            (u64::try_from(length).ok()? > *limit).then_some(Problem::TooLong(length))
        }
        (
            SqlType::Char { .. } | SqlType::Varchar { .. },
            Expr::Value(ValueWithSpan {
                value: Value::DoubleQuotedString(s),
                ..
            }),
        ) if dialect == SqlDialect::MySQL => out_of_range(
            dialect,
            column_type,
            &Expr::value(Value::SingleQuotedString(s.clone())),
        ),
        _ => {
            let literal = signed_number(value)?;
            let fits = if column_type.is_integer() {
                // Literals with a fraction or exponent are rounded first: skipped
                if !literal.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    return None;
                }
                let n: i128 = literal.parse().ok()?;
                let (min, max) = integer_range(dialect, column_type)?;
                (min..=max).contains(&n)
            } else if let SqlType::Decimal {
                precision: Some(precision),
                scale,
            } = column_type
            {
                if literal.contains(['e', 'E']) {
                    return None;
                }
                let integer_part = literal.trim_start_matches('-').split('.').next()?;
                let digits = integer_part.trim_start_matches('0').len();
                // The value is rounded to the scale first, which can only add an
                // integer digit: counting the digits as written never over-reports
                u64::try_from(digits).ok()? <= precision.saturating_sub(scale.unwrap_or(0))
            } else {
                return None;
            };
            (!fits).then_some(Problem::OutOfRange(literal))
        }
    }
}

/// A number literal, with its sign if negated (`-5`, `- 5`)
fn signed_number(value: &Expr) -> Option<String> {
    match value {
        Expr::Value(ValueWithSpan {
            value: Value::Number(n, _),
            ..
        }) => Some(n.clone()),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match expr.as_ref() {
            Expr::Value(ValueWithSpan {
                value: Value::Number(n, _),
                ..
            }) => Some(format!("-{n}")),
            _ => None,
        },
        Expr::Nested(inner) => signed_number(inner),
        _ => None,
    }
}

/// The values an integer column accepts. MySQL integer types may be UNSIGNED,
/// which the catalog doesn't record: they accept the signed minimum to the
/// unsigned maximum here.
fn integer_range(dialect: SqlDialect, column_type: &SqlType) -> Option<(i128, i128)> {
    let bits = match column_type {
        SqlType::TinyInt => 8,
        SqlType::SmallInt => 16,
        SqlType::MediumInt => 24,
        SqlType::Integer => 32,
        SqlType::BigInt => 64,
        _ => return None,
    };
    // PostgreSQL and Redshift have no 1- or 3-byte integers
    if bits % 16 != 0 && dialect != SqlDialect::MySQL {
        return None;
    }
    let min = -(1i128 << (bits - 1));
    let max = if dialect == SqlDialect::MySQL {
        (1i128 << bits) - 1
    } else {
        (1i128 << (bits - 1)) - 1
    };
    Some((min, max))
}

pub(crate) fn collect_referenced_columns(expr: &Expr, columns: &mut HashSet<String>) {
    match expr {
        Expr::Identifier(ident) => {
            columns.insert(ident.value.to_ascii_lowercase());
        }
        Expr::CompoundIdentifier(parts) => {
            if let Some(ident) = parts.last() {
                columns.insert(ident.value.to_ascii_lowercase());
            }
        }
        Expr::Function(func) => {
            if let FunctionArguments::List(ref arg_list) = func.args {
                for arg in &arg_list.args {
                    if let FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) = arg {
                        collect_referenced_columns(e, columns);
                    }
                }
            }
        }
        Expr::UnaryOp { expr, .. } => {
            collect_referenced_columns(expr, columns);
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_referenced_columns(left, columns);
            collect_referenced_columns(right, columns);
        }
        Expr::Nested(e) => {
            collect_referenced_columns(e, columns);
        }
        Expr::InList { expr, list, .. } => {
            collect_referenced_columns(expr, columns);
            for item in list {
                collect_referenced_columns(item, columns);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_referenced_columns(expr, columns);
            collect_referenced_columns(low, columns);
            collect_referenced_columns(high, columns);
        }
        Expr::IsNull(e)
        | Expr::IsNotNull(e)
        | Expr::IsTrue(e)
        | Expr::IsNotTrue(e)
        | Expr::IsFalse(e)
        | Expr::IsNotFalse(e) => {
            collect_referenced_columns(e, columns);
        }
        Expr::Cast { expr, .. } => {
            collect_referenced_columns(expr, columns);
        }
        _ => {}
    }
}

fn is_plain_integer_numeric_or_text(col_type: &SqlType) -> bool {
    col_type.is_integer()
        || matches!(col_type, SqlType::Decimal { .. })
        || matches!(col_type, SqlType::Text | SqlType::Varchar { .. })
}

fn convert_literal_to_column_type(
    expr: &Expr,
    col_type: &SqlType,
    dialect: SqlDialect,
) -> EvalValue {
    if matches!(
        expr,
        Expr::Value(ValueWithSpan {
            value: Value::Null,
            ..
        })
    ) {
        return EvalValue::Null;
    }

    match col_type {
        t if t.is_integer() => {
            // Integer column: convert literal only if it is an integer
            let val = eval_expr(expr, &HashMap::new(), dialect);
            match val {
                EvalValue::Int(n) => EvalValue::Int(n),
                _ => EvalValue::Unknown,
            }
        }
        SqlType::Decimal { scale, .. } => {
            // Plain numeric column: integer or decimal within scale
            let val = eval_expr(expr, &HashMap::new(), dialect);
            match val {
                EvalValue::Int(n) => EvalValue::Decimal {
                    mantissa: n,
                    scale: 0,
                },
                EvalValue::Decimal {
                    mantissa,
                    scale: val_scale,
                } => {
                    if let Some(col_scale) = scale {
                        if u64::from(val_scale) > *col_scale {
                            // Rounded by database; stay quiet
                            return EvalValue::Unknown;
                        }
                    }
                    EvalValue::Decimal {
                        mantissa,
                        scale: val_scale,
                    }
                }
                _ => EvalValue::Unknown,
            }
        }
        SqlType::Text | SqlType::Varchar { .. } => {
            // Skip MySQL string comparisons
            if dialect == SqlDialect::MySQL {
                return EvalValue::Unknown;
            }
            match expr {
                Expr::Value(ValueWithSpan {
                    value: Value::SingleQuotedString(s),
                    ..
                }) => {
                    if dialect.is_psql() {
                        if let SqlType::Varchar {
                            length: Some(limit),
                        } = col_type
                        {
                            let Ok(limit) = usize::try_from(*limit) else {
                                return EvalValue::Unknown;
                            };
                            if s.chars().count() > limit {
                                if s.chars().skip(limit).all(|c| c == ' ') {
                                    return EvalValue::String(s.chars().take(limit).collect());
                                }
                                return EvalValue::Unknown;
                            }
                        }
                    }
                    EvalValue::String(s.clone())
                }
                _ => EvalValue::Unknown,
            }
        }
        _ => EvalValue::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum EvalValue {
    Null,
    Bool(bool),
    Int(i128),
    Decimal { mantissa: i128, scale: u32 },
    String(String),
    Unknown,
}

fn parse_number(s: &str) -> EvalValue {
    if s.contains(['e', 'E']) {
        return EvalValue::Unknown;
    }
    let is_negative = s.starts_with('-');
    let unsigned = s.trim_start_matches(['-', '+']);
    if let Some((int_part, frac_part)) = unsigned.split_once('.') {
        let Ok(scale) = u32::try_from(frac_part.len()) else {
            return EvalValue::Unknown;
        };
        let combined = format!("{int_part}{frac_part}");
        if let Ok(mut mantissa) = combined.parse::<i128>() {
            if is_negative {
                mantissa = -mantissa;
            }
            EvalValue::Decimal { mantissa, scale }
        } else {
            EvalValue::Unknown
        }
    } else if let Ok(n) = s.parse::<i128>() {
        EvalValue::Int(n)
    } else {
        EvalValue::Unknown
    }
}

fn eval_expr(expr: &Expr, env: &HashMap<String, EvalValue>, dialect: SqlDialect) -> EvalValue {
    match expr {
        Expr::Value(ValueWithSpan { value, .. }) => match value {
            Value::Null => EvalValue::Null,
            Value::Boolean(b) => EvalValue::Bool(*b),
            Value::SingleQuotedString(s) => EvalValue::String(s.clone()),
            Value::Number(s, _) => parse_number(s),
            _ => EvalValue::Unknown,
        },
        Expr::Identifier(ident) => env
            .get(&ident.value.to_ascii_lowercase())
            .cloned()
            .unwrap_or(EvalValue::Unknown),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .and_then(|ident| env.get(&ident.value.to_ascii_lowercase()).cloned())
            .unwrap_or(EvalValue::Unknown),
        Expr::Nested(inner) => eval_expr(inner, env, dialect),
        Expr::UnaryOp { op, expr } => match op {
            UnaryOperator::Not => eval_not(&eval_expr(expr, env, dialect)),
            UnaryOperator::Minus => match eval_expr(expr, env, dialect) {
                EvalValue::Int(n) => n.checked_neg().map_or(EvalValue::Unknown, EvalValue::Int),
                EvalValue::Decimal { mantissa, scale } => {
                    mantissa
                        .checked_neg()
                        .map_or(EvalValue::Unknown, |m| EvalValue::Decimal {
                            mantissa: m,
                            scale,
                        })
                }
                EvalValue::Null => EvalValue::Null,
                _ => EvalValue::Unknown,
            },
            UnaryOperator::Plus => match eval_expr(expr, env, dialect) {
                EvalValue::Int(n) => EvalValue::Int(n),
                EvalValue::Decimal { mantissa, scale } => EvalValue::Decimal { mantissa, scale },
                EvalValue::Null => EvalValue::Null,
                _ => EvalValue::Unknown,
            },
            _ => EvalValue::Unknown,
        },
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And => {
                let l = eval_expr(left, env, dialect);
                let r = eval_expr(right, env, dialect);
                eval_and(&l, &r)
            }
            BinaryOperator::Or => {
                let l = eval_expr(left, env, dialect);
                let r = eval_expr(right, env, dialect);
                eval_or(&l, &r)
            }
            BinaryOperator::Eq
            | BinaryOperator::NotEq
            | BinaryOperator::Lt
            | BinaryOperator::LtEq
            | BinaryOperator::Gt
            | BinaryOperator::GtEq => {
                let l = eval_expr(left, env, dialect);
                let r = eval_expr(right, env, dialect);
                eval_comparison(op, &l, &r, dialect)
            }
            BinaryOperator::Plus | BinaryOperator::Minus | BinaryOperator::Multiply => {
                let l = eval_expr(left, env, dialect);
                let r = eval_expr(right, env, dialect);
                eval_arithmetic(op, &l, &r, dialect)
            }
            _ => EvalValue::Unknown,
        },
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let target = eval_expr(expr, env, dialect);
            if target == EvalValue::Unknown {
                return EvalValue::Unknown;
            }
            if let EvalValue::String(_) = &target {
                if dialect == SqlDialect::MySQL {
                    return EvalValue::Unknown;
                }
            }
            let mut has_null = target == EvalValue::Null;
            let mut all_false = true;
            for item in list {
                let item_val = eval_expr(item, env, dialect);
                match eval_comparison(&BinaryOperator::Eq, &target, &item_val, dialect) {
                    EvalValue::Bool(true) => return EvalValue::Bool(!negated),
                    EvalValue::Null => {
                        has_null = true;
                        all_false = false;
                    }
                    EvalValue::Unknown => return EvalValue::Unknown,
                    EvalValue::Bool(false) => {}
                    _ => {}
                }
            }
            if all_false && !has_null {
                EvalValue::Bool(*negated)
            } else if has_null {
                EvalValue::Null
            } else {
                EvalValue::Unknown
            }
        }
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            let val = eval_expr(expr, env, dialect);
            let l = eval_expr(low, env, dialect);
            let h = eval_expr(high, env, dialect);
            let ge = eval_comparison(&BinaryOperator::GtEq, &val, &l, dialect);
            let le = eval_comparison(&BinaryOperator::LtEq, &val, &h, dialect);
            let and_res = eval_and(&ge, &le);
            if *negated {
                eval_not(&and_res)
            } else {
                and_res
            }
        }
        Expr::IsNull(e) => match eval_expr(e, env, dialect) {
            EvalValue::Null => EvalValue::Bool(true),
            EvalValue::Unknown => EvalValue::Unknown,
            _ => EvalValue::Bool(false),
        },
        Expr::IsNotNull(e) => match eval_expr(e, env, dialect) {
            EvalValue::Null => EvalValue::Bool(false),
            EvalValue::Unknown => EvalValue::Unknown,
            _ => EvalValue::Bool(true),
        },
        Expr::IsTrue(e) => match eval_expr(e, env, dialect) {
            EvalValue::Bool(b) => EvalValue::Bool(b),
            EvalValue::Null => EvalValue::Bool(false),
            _ => EvalValue::Unknown,
        },
        Expr::IsNotTrue(e) => match eval_expr(e, env, dialect) {
            EvalValue::Bool(b) => EvalValue::Bool(!b),
            EvalValue::Null => EvalValue::Bool(true),
            _ => EvalValue::Unknown,
        },
        Expr::IsFalse(e) => match eval_expr(e, env, dialect) {
            EvalValue::Bool(b) => EvalValue::Bool(!b),
            EvalValue::Null => EvalValue::Bool(false),
            _ => EvalValue::Unknown,
        },
        Expr::IsNotFalse(e) => match eval_expr(e, env, dialect) {
            EvalValue::Bool(b) => EvalValue::Bool(b),
            EvalValue::Null => EvalValue::Bool(true),
            _ => EvalValue::Unknown,
        },
        _ => EvalValue::Unknown,
    }
}

fn eval_and(l: &EvalValue, r: &EvalValue) -> EvalValue {
    match (l, r) {
        (EvalValue::Bool(false), _) | (_, EvalValue::Bool(false)) => EvalValue::Bool(false),
        (EvalValue::Bool(true), EvalValue::Bool(true)) => EvalValue::Bool(true),
        (EvalValue::Null | EvalValue::Bool(true), EvalValue::Null)
        | (EvalValue::Null, EvalValue::Bool(true)) => EvalValue::Null,
        _ => EvalValue::Unknown,
    }
}

fn eval_or(l: &EvalValue, r: &EvalValue) -> EvalValue {
    match (l, r) {
        (EvalValue::Bool(true), _) | (_, EvalValue::Bool(true)) => EvalValue::Bool(true),
        (EvalValue::Bool(false), EvalValue::Bool(false)) => EvalValue::Bool(false),
        (EvalValue::Null | EvalValue::Bool(false), EvalValue::Null)
        | (EvalValue::Null, EvalValue::Bool(false)) => EvalValue::Null,
        _ => EvalValue::Unknown,
    }
}

fn eval_not(v: &EvalValue) -> EvalValue {
    match v {
        EvalValue::Bool(b) => EvalValue::Bool(!b),
        EvalValue::Null => EvalValue::Null,
        _ => EvalValue::Unknown,
    }
}

fn align_decimals(m1: i128, s1: u32, m2: i128, s2: u32) -> Option<(i128, i128)> {
    match s1.cmp(&s2) {
        std::cmp::Ordering::Equal => Some((m1, m2)),
        std::cmp::Ordering::Less => {
            let diff = s2 - s1;
            if diff > 38 {
                return None;
            }
            let factor = 10i128.checked_pow(diff)?;
            let m1_scaled = m1.checked_mul(factor)?;
            Some((m1_scaled, m2))
        }
        std::cmp::Ordering::Greater => {
            let diff = s1 - s2;
            if diff > 38 {
                return None;
            }
            let factor = 10i128.checked_pow(diff)?;
            let m2_scaled = m2.checked_mul(factor)?;
            Some((m1, m2_scaled))
        }
    }
}

fn eval_comparison(
    op: &BinaryOperator,
    left: &EvalValue,
    right: &EvalValue,
    dialect: SqlDialect,
) -> EvalValue {
    if *left == EvalValue::Null || *right == EvalValue::Null {
        return EvalValue::Null;
    }
    match (left, right) {
        (EvalValue::Int(a), EvalValue::Int(b)) => {
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                BinaryOperator::Lt => a < b,
                BinaryOperator::LtEq => a <= b,
                BinaryOperator::Gt => a > b,
                BinaryOperator::GtEq => a >= b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        (
            EvalValue::Decimal {
                mantissa: m1,
                scale: s1,
            },
            EvalValue::Decimal {
                mantissa: m2,
                scale: s2,
            },
        ) => {
            let Some((a, b)) = align_decimals(*m1, *s1, *m2, *s2) else {
                return EvalValue::Unknown;
            };
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                BinaryOperator::Lt => a < b,
                BinaryOperator::LtEq => a <= b,
                BinaryOperator::Gt => a > b,
                BinaryOperator::GtEq => a >= b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        (
            EvalValue::Int(a),
            EvalValue::Decimal {
                mantissa: m2,
                scale: s2,
            },
        ) => {
            let Some((a, b)) = align_decimals(*a, 0, *m2, *s2) else {
                return EvalValue::Unknown;
            };
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                BinaryOperator::Lt => a < b,
                BinaryOperator::LtEq => a <= b,
                BinaryOperator::Gt => a > b,
                BinaryOperator::GtEq => a >= b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        (
            EvalValue::Decimal {
                mantissa: m1,
                scale: s1,
            },
            EvalValue::Int(b),
        ) => {
            let Some((a, b)) = align_decimals(*m1, *s1, *b, 0) else {
                return EvalValue::Unknown;
            };
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                BinaryOperator::Lt => a < b,
                BinaryOperator::LtEq => a <= b,
                BinaryOperator::Gt => a > b,
                BinaryOperator::GtEq => a >= b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        (EvalValue::String(a), EvalValue::String(b)) => {
            if dialect == SqlDialect::MySQL {
                return EvalValue::Unknown;
            }
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        (EvalValue::Bool(a), EvalValue::Bool(b)) => {
            let res = match op {
                BinaryOperator::Eq => a == b,
                BinaryOperator::NotEq => a != b,
                _ => return EvalValue::Unknown,
            };
            EvalValue::Bool(res)
        }
        _ => EvalValue::Unknown,
    }
}

fn eval_arithmetic(
    op: &BinaryOperator,
    left: &EvalValue,
    right: &EvalValue,
    dialect: SqlDialect,
) -> EvalValue {
    if *left == EvalValue::Null || *right == EvalValue::Null {
        return EvalValue::Null;
    }
    match (left, right) {
        (EvalValue::Int(a), EvalValue::Int(b)) => match op {
            BinaryOperator::Plus => a.checked_add(*b).map_or(EvalValue::Unknown, EvalValue::Int),
            BinaryOperator::Minus => a.checked_sub(*b).map_or(EvalValue::Unknown, EvalValue::Int),
            BinaryOperator::Multiply => {
                a.checked_mul(*b).map_or(EvalValue::Unknown, EvalValue::Int)
            }
            _ => EvalValue::Unknown,
        },
        (
            EvalValue::Decimal {
                mantissa: m1,
                scale: s1,
            },
            EvalValue::Decimal {
                mantissa: m2,
                scale: s2,
            },
        ) => {
            if dialect == SqlDialect::SQLite {
                return EvalValue::Unknown;
            }
            match op {
                BinaryOperator::Plus => {
                    let Some((a, b)) = align_decimals(*m1, *s1, *m2, *s2) else {
                        return EvalValue::Unknown;
                    };
                    let Some(mantissa) = a.checked_add(b) else {
                        return EvalValue::Unknown;
                    };
                    EvalValue::Decimal {
                        mantissa,
                        scale: (*s1).max(*s2),
                    }
                }
                BinaryOperator::Minus => {
                    let Some((a, b)) = align_decimals(*m1, *s1, *m2, *s2) else {
                        return EvalValue::Unknown;
                    };
                    let Some(mantissa) = a.checked_sub(b) else {
                        return EvalValue::Unknown;
                    };
                    EvalValue::Decimal {
                        mantissa,
                        scale: (*s1).max(*s2),
                    }
                }
                BinaryOperator::Multiply => {
                    let Some(mantissa) = m1.checked_mul(*m2) else {
                        return EvalValue::Unknown;
                    };
                    let Some(scale) = s1.checked_add(*s2) else {
                        return EvalValue::Unknown;
                    };
                    EvalValue::Decimal { mantissa, scale }
                }
                _ => EvalValue::Unknown,
            }
        }
        (EvalValue::Int(a), EvalValue::Decimal { .. }) => eval_arithmetic(
            op,
            &EvalValue::Decimal {
                mantissa: *a,
                scale: 0,
            },
            right,
            dialect,
        ),
        (EvalValue::Decimal { .. }, EvalValue::Int(b)) => eval_arithmetic(
            op,
            left,
            &EvalValue::Decimal {
                mantissa: *b,
                scale: 0,
            },
            dialect,
        ),
        _ => EvalValue::Unknown,
    }
}
