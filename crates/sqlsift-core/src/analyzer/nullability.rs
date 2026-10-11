//! Whether an expression may be NULL, for the output columns of a query
//! (`sqlsift describe`). `Some(false)` is only claimed when it is certain:
//! NOT NULL columns, literals, `count(*)`, `IS NULL` tests, ... `Some(true)` means
//! the value may be NULL; `None` that it can't be told.

use sqlparser::ast::{
    BinaryOperator, CastKind, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, Value,
};

use super::resolver::Resolver;
use super::scope::{split_column_ref, ColumnLookup, ColumnMatch};
use crate::ast_ext::ObjectNamePartExt;
use crate::dialect::SqlDialect;
use crate::schema::split_dotted_idents;

/// NULL-ability of an expression that is NULL when any operand is (a strict
/// operator or function): it may be NULL if one may be, certainly not if none can
pub(super) fn strict<I: IntoIterator<Item = Option<bool>>>(operands: I) -> Option<bool> {
    let mut result = Some(false);
    for operand in operands {
        match operand {
            Some(true) => return Some(true),
            Some(false) => {}
            None => result = None,
        }
    }
    result
}

/// NULL-ability of `coalesce(a, b, ...)`: NULL only when every argument is
fn first_non_null<I: IntoIterator<Item = Option<bool>>>(operands: I) -> Option<bool> {
    let mut result = Some(true);
    for operand in operands {
        match operand {
            Some(false) => return Some(false),
            Some(true) => {}
            None => result = None,
        }
    }
    result
}

/// NULL-ability of a column of a set operation (`UNION`, ...): NULL if either
/// branch's column may be
pub(super) fn either(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

impl Resolver<'_> {
    /// NULL-ability of an output column computed by `expr`, only inferred when
    /// describing (checks don't use it)
    pub(super) fn output_nullable(&self, expr: &Expr) -> Option<bool> {
        if self.parameter_types.is_some() {
            self.infer_nullable(expr)
        } else {
            None
        }
    }

    /// Whether `expr` may be NULL (see the module docs)
    pub(super) fn infer_nullable(&self, expr: &Expr) -> Option<bool> {
        match expr {
            Expr::Value(value) => match &value.value {
                Value::Null => Some(true),
                Value::Placeholder(_) => None,
                _ => Some(false),
            },
            Expr::Identifier(ident) => match self.scope.column(&ident.value, self.dialect) {
                ColumnLookup::Found(column) => column.nullable,
                _ => None,
            },
            Expr::CompoundIdentifier(_) if self.dialect.struct_field_paths() => None,
            Expr::CompoundIdentifier(parts) => {
                let parts = split_dotted_idents(parts);
                let (schema, table, column) = split_column_ref(&parts)?;
                let relation = self.scope.qualified_relation(self.catalog, schema, table)?;
                match relation.column(&column.value, self.dialect) {
                    ColumnMatch::Yes(column) => column.nullable,
                    _ => None,
                }
            }
            Expr::Nested(inner)
            | Expr::Collate { expr: inner, .. }
            | Expr::UnaryOp { expr: inner, .. }
            | Expr::Extract { expr: inner, .. }
            | Expr::Ceil { expr: inner, .. }
            | Expr::Floor { expr: inner, .. } => self.infer_nullable(inner),
            Expr::Cast {
                kind, expr: inner, ..
            } => match kind {
                // TRY_CAST / SAFE_CAST give NULL for a value that doesn't convert
                CastKind::TryCast | CastKind::SafeCast => Some(true),
                CastKind::Cast | CastKind::DoubleColon => self.infer_nullable(inner),
            },
            Expr::IsNull(_)
            | Expr::IsNotNull(_)
            | Expr::IsTrue(_)
            | Expr::IsFalse(_)
            | Expr::IsNotTrue(_)
            | Expr::IsNotFalse(_)
            | Expr::IsUnknown(_)
            | Expr::IsNotUnknown(_)
            | Expr::IsDistinctFrom(..)
            | Expr::IsNotDistinctFrom(..)
            | Expr::Exists { .. }
            | Expr::TypedString(_)
            | Expr::Interval(_)
            | Expr::Array(_) => Some(false),
            Expr::BinaryOp { left, op, right } => self.binary_nullable(left, op, right),
            Expr::InList { expr, list, .. } => strict(
                std::iter::once(expr.as_ref())
                    .chain(list)
                    .map(|e| self.infer_nullable(e)),
            ),
            Expr::Between {
                expr, low, high, ..
            } => strict([expr, low, high].map(|e| self.infer_nullable(e))),
            Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
                strict([expr, pattern].map(|e| self.infer_nullable(e)))
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => strict(
                std::iter::once(expr.as_ref())
                    .chain(substring_from.as_deref())
                    .chain(substring_for.as_deref())
                    .map(|e| self.infer_nullable(e)),
            ),
            Expr::Trim {
                expr, trim_what, ..
            } => strict(
                std::iter::once(expr.as_ref())
                    .chain(trim_what.as_deref())
                    .map(|e| self.infer_nullable(e)),
            ),
            Expr::Position { expr, r#in } => strict([expr, r#in].map(|e| self.infer_nullable(e))),
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => strict([timestamp, time_zone].map(|e| self.infer_nullable(e))),
            Expr::Case {
                conditions,
                else_result,
                ..
            } => {
                // Without ELSE, a row no branch matches is NULL
                let Some(else_result) = else_result.as_deref() else {
                    return Some(true);
                };
                strict(
                    conditions
                        .iter()
                        .map(|c| &c.result)
                        .chain(std::iter::once(else_result))
                        .map(|e| self.infer_nullable(e)),
                )
            }
            // A scalar subquery is NULL when it returns no row
            Expr::Subquery(_) => Some(true),
            Expr::Function(func) => self.function_nullable(func),
            _ => None,
        }
    }

    fn binary_nullable(&self, left: &Expr, op: &BinaryOperator, right: &Expr) -> Option<bool> {
        use BinaryOperator::{
            And, Divide, Eq, Gt, GtEq, Lt, LtEq, Minus, Modulo, Multiply, NotEq, Or, Plus,
            Spaceship, StringConcat,
        };
        let operands = || strict([left, right].map(|e| self.infer_nullable(e)));
        match op {
            // MySQL's NULL-safe equality
            Spaceship => Some(false),
            // MySQL and SQLite give NULL for a division by zero
            Divide | Modulo if matches!(self.dialect, SqlDialect::MySQL | SqlDialect::SQLite) => {
                Some(true)
            }
            Plus | Minus | Multiply | Divide | Modulo | Eq | NotEq | Lt | LtEq | Gt | GtEq
            | And | Or => operands(),
            // MySQL's CONCAT operator is OR (`||`) unless PIPES_AS_CONCAT
            StringConcat if self.dialect != SqlDialect::MySQL => operands(),
            // JSON access (`->`, `->>`) is NULL for a missing key; others aren't known
            _ => operands().filter(|&nullable| nullable),
        }
    }

    fn function_nullable(&self, func: &sqlparser::ast::Function) -> Option<bool> {
        let name = func.name.0.last()?.ident().value.to_lowercase();
        let args: Vec<Option<bool>> = match &func.args {
            FunctionArguments::List(list) => list
                .args
                .iter()
                .map(|arg| match arg {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(e))
                    | FunctionArg::Named {
                        arg: FunctionArgExpr::Expr(e),
                        ..
                    } => self.infer_nullable(e),
                    _ => None,
                })
                .collect(),
            FunctionArguments::None => Vec::new(),
            FunctionArguments::Subquery(_) => return None,
        };
        match name.as_str() {
            // Counts and ranks are never NULL
            "count" | "row_number" | "rank" | "dense_rank" | "percent_rank" | "cume_dist"
            | "ntile" => Some(false),
            // Current time and generated values
            "now"
            | "current_timestamp"
            | "current_date"
            | "current_time"
            | "localtimestamp"
            | "localtime"
            | "transaction_timestamp"
            | "statement_timestamp"
            | "clock_timestamp"
            | "gen_random_uuid"
            | "uuid_generate_v4"
            | "random" => Some(false),
            // NULL only when every argument is
            "coalesce" | "ifnull" | "nvl" => first_non_null(args),
            "greatest" | "least" if self.dialect.is_psql() => first_non_null(args),
            // Aggregates over no rows (or only NULLs), and window functions reaching
            // past the partition, are NULL
            "sum" | "avg" | "min" | "max" | "array_agg" | "string_agg" | "group_concat"
            | "json_agg" | "jsonb_agg" | "json_object_agg" | "jsonb_object_agg" | "bool_and"
            | "bool_or" | "every" | "bit_and" | "bit_or" | "stddev" | "stddev_pop"
            | "stddev_samp" | "variance" | "var_pop" | "var_samp" | "lag" | "lead"
            | "first_value" | "last_value" | "nth_value" | "nullif" => Some(true),
            // Strict scalar functions: NULL when an argument is
            // MySQL and SQLite give NULL for arguments out of the domain
            "sqrt" | "power" | "pow" | "mod" | "exp" | "ln" | "log" | "log10" | "log2"
                if !self.dialect.is_psql() =>
            {
                Some(true)
            }
            // Strict scalar functions: NULL when an argument is
            "upper" | "lower" | "length" | "char_length" | "character_length" | "octet_length"
            | "trim" | "ltrim" | "rtrim" | "btrim" | "abs" | "round" | "ceil" | "ceiling"
            | "floor" | "trunc" | "sign" | "sqrt" | "power" | "mod" | "exp" | "ln" | "log"
            | "substr" | "replace" | "lpad" | "rpad" | "left" | "right" | "reverse" | "repeat"
            | "md5" | "initcap" | "date_trunc" | "date_part" | "to_char" | "age"
                if !args.is_empty() =>
            {
                strict(args)
            }
            _ => None,
        }
    }
}
