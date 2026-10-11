//! Types of query parameters (`$1`, `?`, `:name`), inferred from where they are
//! used: compared with a column, inserted into or assigned to a column, cast,
//! in an IN list, BETWEEN bounds, LIKE patterns, `= ANY(...)` and LIMIT / OFFSET.
//! Only collected for [`Analyzer::describe`](super::Analyzer::describe).

use std::collections::HashMap;

use sqlparser::ast::{Expr, Value, ValueWithSpan};

use super::resolver::Resolver;
use super::scope::ExpressionType;
use crate::types::SqlType;

/// Types inferred for the placeholders of a statement, by the location
/// ((line, column) relative to the statement's text) of each occurrence
#[derive(Debug, Default)]
pub(super) struct ParameterTypes {
    types: HashMap<(usize, usize), ParameterType>,
}

/// The type of one placeholder occurrence
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ParameterType {
    Known(SqlType),
    /// Used where different types are expected
    Conflicting,
}

impl ParameterType {
    /// The type of a parameter used where both `self` and `other` are expected
    pub(super) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Known(a), Self::Known(b)) if a == b => Self::Known(a),
            _ => Self::Conflicting,
        }
    }
}

impl ParameterTypes {
    /// Inferred type of the placeholder at `location`
    pub(super) fn get(&self, location: (usize, usize)) -> Option<&ParameterType> {
        self.types.get(&location)
    }
}

/// Location of a placeholder expression (through parentheses)
pub(super) fn placeholder_location(expr: &Expr) -> Option<(usize, usize)> {
    match expr {
        Expr::Nested(inner) => placeholder_location(inner),
        Expr::Value(ValueWithSpan {
            value: Value::Placeholder(_),
            span,
        }) if span.start.line > 0 => Some((span.start.line as usize, span.start.column as usize)),
        _ => None,
    }
}

impl Resolver<'_> {
    /// Record that `expr`, if it is a placeholder, takes a value of type `ty`
    pub(super) fn note_parameter(&mut self, expr: &Expr, ty: &ExpressionType) {
        let Some(parameters) = self.parameter_types.as_mut() else {
            return;
        };
        let Some(location) = placeholder_location(expr) else {
            return;
        };
        let ExpressionType::Known(ty) = ty else {
            return;
        };
        if matches!(ty, SqlType::Unknown | SqlType::SemiStructured(_)) {
            return;
        }
        let ty = ParameterType::Known(ty.clone());
        let merged = match parameters.types.remove(&location) {
            Some(previous) => previous.merge(ty),
            None => ty,
        };
        parameters.types.insert(location, merged);
    }

    /// [`Resolver::note_parameter`] with the type of `other` (the two operands of
    /// a comparison take the same type)
    pub(super) fn note_compared_parameters(
        &mut self,
        left: &Expr,
        left_type: &ExpressionType,
        right: &Expr,
        right_type: &ExpressionType,
    ) {
        self.note_parameter(left, right_type);
        self.note_parameter(right, left_type);
    }

    /// A LIKE / ILIKE pattern placeholder matching a string `expr` is text
    pub(super) fn note_pattern_parameter(&mut self, expr: &Expr, pattern: &Expr) {
        if !self.is_typed_parameter(pattern) {
            return;
        }
        if let ExpressionType::Known(ty) = self.infer_expr_type(expr) {
            if ty.is_string() {
                self.note_parameter(pattern, &ExpressionType::Known(SqlType::Text));
            }
        }
    }

    /// Whether parameter types are collected and `expr` is a placeholder (so that
    /// inferring the type it takes is worth it)
    pub(super) fn is_typed_parameter(&self, expr: &Expr) -> bool {
        self.parameter_types.is_some() && placeholder_location(expr).is_some()
    }
}
