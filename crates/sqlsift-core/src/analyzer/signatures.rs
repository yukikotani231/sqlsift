//! E0027 wrong-argument-type: a built-in function or operator called with an
//! argument type it has no version for (PostgreSQL)
//!
//! `sum(text)`, `avg(date)`, `lower(integer)`, `length(date)`, `id LIKE '1%'` on
//! an integer: PostgreSQL has no implicit cast to text or numbers for these, so
//! it reports "function ... does not exist" / "operator does not exist". Only
//! arguments of a known type are checked, and nothing is reported for a name the
//! schema defines a function of (it may overload the built-in one) or when the
//! schema defines operators.

use sqlparser::ast::{Expr, Function, FunctionArg, FunctionArgExpr, FunctionArguments};

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
        | "rtrim" | "btrim" | "reverse" | "md5" => Accepts::Text,
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
            [schema, name] if schema.value.eq_ignore_ascii_case("pg_catalog") => name,
            _ => return,
        };
        if name.quote_style.is_some() {
            return;
        }
        let name = name.value.to_lowercase();
        let Some(accepts) = builtin(&name) else {
            return;
        };
        if self.catalog.user_functions.contains(&name) {
            return;
        }
        let FunctionArguments::List(list) = &func.args else {
            return;
        };
        let [FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))] = list.args.as_slice() else {
            return;
        };
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
        let type_name = ty.display_name();
        let (takes, help) = match accepts {
            Accepts::NumberOrInterval => (
                "numbers or intervals",
                format!("{name}() needs a number: cast the argument or use another aggregate"),
            ),
            Accepts::Text => (
                "text",
                format!("Cast the argument to text: {name}(x::text)"),
            ),
        };
        let mut diag = Diagnostic::error(
            DiagnosticKind::WrongArgumentType,
            format!("Function {name}({type_name}) does not exist: {name}() takes {takes}"),
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
