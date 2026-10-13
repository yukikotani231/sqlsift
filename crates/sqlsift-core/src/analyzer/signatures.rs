//! E0027 wrong-argument-type: a built-in function or operator called with an
//! argument type it has no version for (PostgreSQL)
//!
//! `sum(text)`, `avg(date)`, `lower(integer)`, `length(date)`, `id LIKE '1%'` on
//! an integer: PostgreSQL has no implicit cast to text or numbers for these, so
//! it reports "function ... does not exist" / "operator does not exist". Only
//! arguments of a known type are checked, and nothing is reported for a name the
//! schema defines a function of (it may overload the built-in one) or when the
//! schema defines operators.

use sqlsift_ast::{Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments};

use crate::ast_ext::ObjectNamePartExt;
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind};
use crate::types::SqlType;

use super::resolver::Resolver;
use super::scope::ExpressionType;

/// What a built-in function's argument must be
#[derive(Clone, Copy)]
enum Accepts {
    /// Numbers and intervals (`sum`, `avg`)
    NumberOrInterval,
    /// Character strings (and `bytea` for `length` / `md5`)
    Text,
}

fn builtin(name: &str) -> Option<Accepts> {
    Some(match name {
        "sum" | "avg" => Accepts::NumberOrInterval,
        "lower" | "upper" | "initcap" | "length" | "char_length" | "character_length" | "ltrim"
        | "rtrim" | "btrim" | "reverse" | "md5" | "lpad" | "rpad" | "left" | "right"
        | "split_part" | "strpos" | "starts_with" | "replace" | "repeat" | "translate" => {
            Accepts::Text
        }
        _ => return None,
    })
}

/// Types that are never text and have no implicit cast to it
fn is_non_text(ty: &SqlType) -> bool {
    ty.is_numeric()
        || matches!(
            ty,
            SqlType::Boolean
                | SqlType::Date
                | SqlType::Time { .. }
                | SqlType::Timestamp { .. }
                | SqlType::Interval
                | SqlType::Uuid
                | SqlType::Json
                | SqlType::Jsonb
        )
}

/// Types `sum` / `avg` have no version for
fn is_non_numeric(ty: &SqlType) -> bool {
    matches!(
        ty,
        SqlType::Char { .. }
            | SqlType::Varchar { .. }
            | SqlType::Text
            | SqlType::Boolean
            | SqlType::Date
            | SqlType::Time { .. }
            | SqlType::Timestamp { .. }
            | SqlType::Uuid
            | SqlType::Json
            | SqlType::Jsonb
    )
}

/// Whether an expression needs parentheses before appending a cast `::type`
fn needs_parens_for_cast(expr: &Expr) -> bool {
    !matches!(
        expr,
        Expr::Identifier(_) | Expr::CompoundIdentifier(_) | Expr::Value(_) | Expr::Nested(_)
    )
}

impl Resolver<'_> {
    /// Report a built-in function called with an argument type it doesn't accept
    pub(super) fn check_function_signature(&mut self, func: &Function) {
        if self.dialect != SqlDialect::PostgreSQL {
            return;
        }
        // Unqualified or pg_catalog names only: a function of another schema is
        // not the built-in one
        let name = match func.name.0.as_slice() {
            [name] => name,
            [schema, name] if schema.ident().value.eq_ignore_ascii_case("pg_catalog") => name,
            _ => return,
        };
        if name.ident().quote_style.is_some() {
            return;
        }
        let name = name.ident().value.to_lowercase();
        let Some(accepts) = builtin(&name) else {
            return;
        };
        if self.catalog.user_functions.contains(&name) {
            return;
        }
        let FunctionArguments::List(list) = &func.args else {
            return;
        };
        let Some(first_arg) = list.args.first() else {
            return;
        };
        let FunctionArg::Unnamed(FunctionArgExpr::Expr(arg)) = first_arg else {
            return;
        };
        // For NumberOrInterval (aggregates sum/avg), only 1-argument calls are checked
        if matches!(accepts, Accepts::NumberOrInterval) && list.args.len() != 1 {
            return;
        }
        let ExpressionType::Known(ty) = self.infer_expr_type(arg) else {
            return;
        };
        let wrong = match accepts {
            Accepts::NumberOrInterval => is_non_numeric(&ty),
            Accepts::Text => is_non_text(&ty),
        };
        if !wrong {
            return;
        }
        let (takes, help) = match accepts {
            Accepts::NumberOrInterval => (
                "numbers or intervals",
                format!("{name}() needs a number: cast the argument or use another aggregate"),
            ),
            Accepts::Text => {
                let mut suggested: Vec<String> = Vec::with_capacity(list.args.len());
                for (i, a) in list.args.iter().enumerate() {
                    match a {
                        FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => {
                            if i == 0 {
                                if needs_parens_for_cast(e) {
                                    suggested.push(format!("({e})::text"));
                                } else {
                                    suggested.push(format!("{e}::text"));
                                }
                            } else {
                                suggested.push(format!("{e}"));
                            }
                        }
                        _ => suggested.push("...".to_string()),
                    }
                }
                (
                    "text",
                    format!(
                        "Cast the argument to text: {name}({})",
                        suggested.join(", ")
                    ),
                )
            }
        };
        let arg_types: Vec<String> = list
            .args
            .iter()
            .map(|a| match a {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => match self.infer_expr_type(e) {
                    ExpressionType::Known(t) => t.display_name(),
                    _ => "unknown".to_string(),
                },
                _ => "unknown".to_string(),
            })
            .collect();
        let types_str = arg_types.join(", ");
        let mut diag = Diagnostic::error(
            DiagnosticKind::WrongArgumentType,
            format!("Function {name}({types_str}) does not exist: {name}() takes {takes}"),
        )
        .with_help(help);
        diag.span = super::type_check::located_span(arg);
        self.diagnostics.push(diag);
    }

    /// Report `x LIKE ...` / `ILIKE` where `x` has a type that isn't text
    pub(super) fn check_like_operand(&mut self, expr: &Expr, operator: &str) {
        if self.dialect != SqlDialect::PostgreSQL || self.catalog.user_operators {
            return;
        }
        let ExpressionType::Known(ty) = self.infer_expr_type(expr) else {
            return;
        };
        if !is_non_text(&ty) {
            return;
        }
        let mut diag = Diagnostic::error(
            DiagnosticKind::WrongArgumentType,
            format!(
                "Operator {operator} does not exist for {}: it matches text",
                ty.display_name()
            ),
        )
        .with_help("Cast the value to text: x::text LIKE '...'");
        diag.span = super::type_check::located_span(expr);
        self.diagnostics.push(diag);
    }
}
