//! Locations of nodes, computed from the locations of their parts the way sqlparser's
//! `Spanned` computes them (an expression's span leaves out most keywords, `(x)` is the
//! span of `x`, ...), so that diagnostics point at the same text whichever parser read
//! the statement.

use crate::{
    AccessExpr, Array, Assignment, AssignmentTarget, ConflictTarget, Cte, Delete, DoUpdate,
    ExceptSelectItem, ExcludeSelectItem, Expr, ExprWithAlias, Fetch, FromTable, Function,
    FunctionArg, FunctionArgExpr, FunctionArgumentClause, FunctionArgumentList, FunctionArguments,
    GroupByExpr, Insert, Join, JoinConstraint, JoinOperator, JsonPath, JsonPathElem, LateralView,
    LimitClause, NamedWindowDefinition, ObjectName, ObjectNamePart, Offset, OnConflict,
    OnConflictAction, OnInsert, OrderBy, OrderByExpr, OrderByKind, Parens, PivotValueSource, Query,
    RenameSelectItem, ReplaceSelectItem, Select, SelectInto, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, Span, Statement, Subscript, TableAlias, TableFactor,
    TableObject, TableWithJoins, Update, UpdateTableFromKind, ValueWithSpan, Values,
    WildcardAdditionalOptions, With,
};

/// A node with a location.
pub trait Spanned {
    /// The text the node covers (empty if unknown).
    fn span(&self) -> Span;
}

fn union<I: IntoIterator<Item = Span>>(spans: I) -> Span {
    Span::union_iter(spans)
}

impl<T> Spanned for Parens<T> {
    fn span(&self) -> Span {
        self.opening_token.0.span.union(&self.closing_token.0.span)
    }
}

impl Spanned for ValueWithSpan {
    fn span(&self) -> Span {
        self.span
    }
}

impl Spanned for ObjectName {
    fn span(&self) -> Span {
        union(self.0.iter().map(Spanned::span))
    }
}

impl Spanned for ObjectNamePart {
    fn span(&self) -> Span {
        match self {
            ObjectNamePart::Identifier(ident) => ident.span,
            ObjectNamePart::Function(function) => function
                .name
                .span
                .union(&union(function.args.iter().map(Spanned::span))),
        }
    }
}

impl Spanned for Query {
    fn span(&self) -> Span {
        union(
            self.with
                .iter()
                .map(Spanned::span)
                .chain(std::iter::once(self.body.span()))
                .chain(self.order_by.as_ref().map(Spanned::span))
                .chain(self.limit_clause.as_ref().map(Spanned::span))
                .chain(self.fetch.as_ref().map(Spanned::span)),
        )
    }
}

impl Spanned for LimitClause {
    fn span(&self) -> Span {
        match self {
            LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            } => union(
                limit
                    .iter()
                    .map(Spanned::span)
                    .chain(offset.as_ref().map(Spanned::span))
                    .chain(limit_by.iter().map(Spanned::span)),
            ),
            LimitClause::OffsetCommaLimit { offset, limit } => offset.span().union(&limit.span()),
        }
    }
}

impl Spanned for Offset {
    fn span(&self) -> Span {
        self.value.span()
    }
}

impl Spanned for Fetch {
    fn span(&self) -> Span {
        self.quantity.as_ref().map_or(Span::empty(), Spanned::span)
    }
}

impl Spanned for With {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.with_token.0.span)
                .chain(self.cte_tables.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for Cte {
    fn span(&self) -> Span {
        union(
            [self.alias.span(), self.query.span()]
                .into_iter()
                .chain(self.from.iter().map(|from| from.span))
                .chain(std::iter::once(self.closing_paren_token.0.span)),
        )
    }
}

impl Spanned for SetExpr {
    fn span(&self) -> Span {
        match self {
            SetExpr::Select(select) => select.span(),
            SetExpr::Query(query) => query.span(),
            SetExpr::SetOperation { left, right, .. } => left.span().union(&right.span()),
            SetExpr::Values(values) => values.span(),
            SetExpr::Insert(statement)
            | SetExpr::Update(statement)
            | SetExpr::Delete(statement)
            | SetExpr::Merge(statement) => statement.span(),
            SetExpr::Table(_) => Span::empty(),
        }
    }
}

impl Spanned for Values {
    fn span(&self) -> Span {
        match self.rows.as_slice() {
            [] => Span::empty(),
            [row] => row.span(),
            [first, .., last] => first.span().union(&last.span()),
        }
    }
}

impl Spanned for Statement {
    fn span(&self) -> Span {
        match self {
            Statement::Query(query) => query.span(),
            Statement::Insert(insert) => insert.span(),
            Statement::Update(update) => update.span(),
            Statement::Delete(delete) => delete.span(),
            Statement::Merge(merge) => merge.table.span().union(&merge.on.span()),
            Statement::CreateTable(create) => union(
                std::iter::once(create.name.span())
                    .chain(create.columns.iter().map(|c| c.name.span))
                    .chain(create.query.as_ref().map(|q| q.span())),
            ),
            Statement::CreateView(view) => view.name.span().union(&view.query.span()),
            Statement::CreateIndex(index) => index.table_name.span(),
            Statement::CreateType { name, .. } => name.span(),
            Statement::CreateTrigger(trigger) => trigger.name.span(),
            Statement::AlterTable(alter) => alter.name.span(),
            Statement::AlterType(alter) => alter.name.span(),
            Statement::Drop { names, .. } => union(names.iter().map(Spanned::span)),
            Statement::Set(_) | Statement::Use(_) | Statement::Declare { .. } => Span::empty(),
            Statement::Other(other) => other.span,
        }
    }
}

impl Spanned for Insert {
    fn span(&self) -> Span {
        union(
            [self.insert_token.0.span, self.table.span()]
                .into_iter()
                .chain(self.table_alias.iter().map(|a| a.alias.span))
                .chain(self.columns.iter().map(Spanned::span))
                .chain(self.source.as_ref().map(|q| q.span()))
                .chain(self.assignments.iter().map(Spanned::span))
                .chain(self.partitioned.iter().flatten().map(Spanned::span))
                .chain(self.after_columns.iter().map(|i| i.span))
                .chain(self.on.as_ref().map(Spanned::span))
                .chain(self.returning.iter().flatten().map(Spanned::span)),
        )
    }
}

impl Spanned for TableObject {
    fn span(&self) -> Span {
        match self {
            TableObject::TableName(name) => name.span(),
            TableObject::TableFunction(function) => function.span(),
            TableObject::TableQuery(query) => query.span(),
        }
    }
}

impl Spanned for OnInsert {
    fn span(&self) -> Span {
        match self {
            OnInsert::DuplicateKeyUpdate(assignments) => {
                union(assignments.iter().map(Spanned::span))
            }
            OnInsert::OnConflict(on_conflict) => on_conflict.span(),
        }
    }
}

impl Spanned for OnConflict {
    fn span(&self) -> Span {
        self.action
            .span()
            .union_opt(&self.conflict_target.as_ref().map(Spanned::span))
    }
}

impl Spanned for ConflictTarget {
    fn span(&self) -> Span {
        match self {
            ConflictTarget::Columns(columns) => union(columns.iter().map(|i| i.span)),
            ConflictTarget::OnConstraint(name) => name.span(),
        }
    }
}

impl Spanned for OnConflictAction {
    fn span(&self) -> Span {
        match self {
            OnConflictAction::DoNothing => Span::empty(),
            OnConflictAction::DoUpdate(update) => update.span(),
        }
    }
}

impl Spanned for DoUpdate {
    fn span(&self) -> Span {
        union(
            self.assignments
                .iter()
                .map(Spanned::span)
                .chain(self.selection.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for Assignment {
    fn span(&self) -> Span {
        self.target.span().union(&self.value.span())
    }
}

impl Spanned for AssignmentTarget {
    fn span(&self) -> Span {
        match self {
            AssignmentTarget::ColumnName(name) => name.span(),
            AssignmentTarget::Tuple(names) => union(names.iter().map(Spanned::span)),
        }
    }
}

impl Spanned for Update {
    fn span(&self) -> Span {
        union(
            [self.table.span(), self.update_token.0.span]
                .into_iter()
                .chain(self.assignments.iter().map(Spanned::span))
                .chain(self.from.iter().map(Spanned::span))
                .chain(self.selection.iter().map(Spanned::span))
                .chain(self.returning.iter().flatten().map(Spanned::span))
                .chain(self.order_by.iter().map(Spanned::span))
                .chain(self.limit.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for UpdateTableFromKind {
    fn span(&self) -> Span {
        match self {
            UpdateTableFromKind::BeforeSet(from) | UpdateTableFromKind::AfterSet(from) => {
                union(from.iter().map(Spanned::span))
            }
        }
    }
}

impl Spanned for Delete {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.delete_token.0.span).chain(
                self.tables
                    .iter()
                    .map(Spanned::span)
                    .chain(std::iter::once(self.from.span()))
                    .chain(
                        self.using
                            .iter()
                            .map(|using| union(using.iter().map(Spanned::span))),
                    )
                    .chain(self.selection.iter().map(Spanned::span))
                    .chain(self.returning.iter().flatten().map(Spanned::span))
                    .chain(self.order_by.iter().map(Spanned::span))
                    .chain(self.limit.iter().map(Spanned::span)),
            ),
        )
    }
}

impl Spanned for FromTable {
    fn span(&self) -> Span {
        match self {
            FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => {
                union(from.iter().map(Spanned::span))
            }
        }
    }
}

impl Spanned for OrderBy {
    fn span(&self) -> Span {
        match &self.kind {
            OrderByKind::All(_) => Span::empty(),
            OrderByKind::Expressions(exprs) => union(exprs.iter().map(Spanned::span)),
        }
    }
}

impl Spanned for OrderByExpr {
    fn span(&self) -> Span {
        self.expr.span()
    }
}

impl Spanned for GroupByExpr {
    fn span(&self) -> Span {
        match self {
            GroupByExpr::All(_) => Span::empty(),
            GroupByExpr::Expressions(exprs, _) => union(exprs.iter().map(Spanned::span)),
        }
    }
}

impl Spanned for Select {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.select_token.0.span)
                .chain(self.projection.iter().map(Spanned::span))
                .chain(self.into.iter().map(Spanned::span))
                .chain(self.from.iter().map(Spanned::span))
                .chain(self.lateral_views.iter().map(Spanned::span))
                .chain(self.prewhere.iter().map(Spanned::span))
                .chain(self.selection.iter().map(Spanned::span))
                .chain(self.connect_by.iter().map(Spanned::span))
                .chain(std::iter::once(self.group_by.span()))
                .chain(self.cluster_by.iter().map(Spanned::span))
                .chain(self.distribute_by.iter().map(Spanned::span))
                .chain(self.sort_by.iter().map(Spanned::span))
                .chain(self.having.iter().map(Spanned::span))
                .chain(self.named_window.iter().map(Spanned::span))
                .chain(self.qualify.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for SelectInto {
    fn span(&self) -> Span {
        union(self.targets.iter().map(Spanned::span))
    }
}

impl Spanned for LateralView {
    fn span(&self) -> Span {
        union(
            [self.lateral_view.span(), self.lateral_view_name.span()]
                .into_iter()
                .chain(self.lateral_col_alias.iter().map(|i| i.span)),
        )
    }
}

impl Spanned for NamedWindowDefinition {
    fn span(&self) -> Span {
        self.0.span
    }
}

impl Spanned for SelectItem {
    fn span(&self) -> Span {
        match self {
            SelectItem::UnnamedExpr(expr) => expr.span(),
            SelectItem::ExprWithAlias { expr, alias } => expr.span().union(&alias.span),
            SelectItem::ExprWithAliases { expr, aliases } => {
                union(std::iter::once(expr.span()).chain(aliases.iter().map(|i| i.span)))
            }
            SelectItem::QualifiedWildcard(kind, options) => kind.span().union(&options.span()),
            SelectItem::Wildcard(options) => options.span(),
        }
    }
}

impl Spanned for SelectItemQualifiedWildcardKind {
    fn span(&self) -> Span {
        match self {
            SelectItemQualifiedWildcardKind::ObjectName(name) => name.span(),
            SelectItemQualifiedWildcardKind::Expr(expr) => expr.span(),
        }
    }
}

impl Spanned for WildcardAdditionalOptions {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.wildcard_token.0.span)
                .chain(self.opt_exclude.as_ref().map(Spanned::span))
                .chain(self.opt_rename.as_ref().map(Spanned::span))
                .chain(self.opt_replace.as_ref().map(Spanned::span))
                .chain(self.opt_except.as_ref().map(Spanned::span))
                .chain(self.opt_alias.as_ref().map(|i| i.span)),
        )
    }
}

impl Spanned for ExcludeSelectItem {
    fn span(&self) -> Span {
        match self {
            ExcludeSelectItem::Single(name) => name.span(),
            ExcludeSelectItem::Multiple(names) => union(names.iter().map(Spanned::span)),
        }
    }
}

impl Spanned for RenameSelectItem {
    fn span(&self) -> Span {
        match self {
            RenameSelectItem::Single(item) => item.ident.span.union(&item.alias.span),
            RenameSelectItem::Multiple(items) => {
                union(items.iter().map(|i| i.ident.span.union(&i.alias.span)))
            }
        }
    }
}

impl Spanned for ExceptSelectItem {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.first_element.span)
                .chain(self.additional_elements.iter().map(|i| i.span)),
        )
    }
}

impl Spanned for ReplaceSelectItem {
    fn span(&self) -> Span {
        union(
            self.items
                .iter()
                .map(|item| item.expr.span().union(&item.column_name.span)),
        )
    }
}

impl Spanned for TableWithJoins {
    fn span(&self) -> Span {
        union(std::iter::once(self.relation.span()).chain(self.joins.iter().map(Spanned::span)))
    }
}

impl Spanned for Join {
    fn span(&self) -> Span {
        self.relation.span().union(&self.join_operator.span())
    }
}

impl Spanned for JoinOperator {
    fn span(&self) -> Span {
        match self {
            JoinOperator::Join(c)
            | JoinOperator::Inner(c)
            | JoinOperator::Left(c)
            | JoinOperator::LeftOuter(c)
            | JoinOperator::Right(c)
            | JoinOperator::RightOuter(c)
            | JoinOperator::FullOuter(c)
            | JoinOperator::CrossJoin(c)
            | JoinOperator::Semi(c)
            | JoinOperator::LeftSemi(c)
            | JoinOperator::RightSemi(c)
            | JoinOperator::Anti(c)
            | JoinOperator::LeftAnti(c)
            | JoinOperator::RightAnti(c)
            | JoinOperator::StraightJoin(c) => c.span(),
            JoinOperator::AsOf {
                match_condition,
                constraint,
            } => match_condition.span().union(&constraint.span()),
            JoinOperator::CrossApply
            | JoinOperator::OuterApply
            | JoinOperator::ArrayJoin
            | JoinOperator::LeftArrayJoin
            | JoinOperator::InnerArrayJoin => Span::empty(),
        }
    }
}

impl Spanned for JoinConstraint {
    fn span(&self) -> Span {
        match self {
            JoinConstraint::On(expr) => expr.span(),
            JoinConstraint::Using(names) => union(names.iter().map(Spanned::span)),
            JoinConstraint::Natural | JoinConstraint::None => Span::empty(),
        }
    }
}

impl Spanned for TableAlias {
    fn span(&self) -> Span {
        union(
            std::iter::once(self.name.span)
                .chain(self.columns.iter().map(|c| c.name.span))
                .chain(self.at.iter().map(|at| at.span)),
        )
    }
}

impl Spanned for TableFactor {
    fn span(&self) -> Span {
        let alias = |alias: &Option<TableAlias>| alias.as_ref().map(Spanned::span);
        match self {
            TableFactor::Table {
                name,
                alias: table_alias,
                ..
            } => union(
                name.0
                    .iter()
                    .map(Spanned::span)
                    .chain(table_alias.as_ref().map(|a| {
                        union(
                            std::iter::once(a.name.span)
                                .chain(a.columns.iter().map(|c| c.name.span)),
                        )
                    })),
            ),
            TableFactor::Derived {
                subquery,
                alias: table_alias,
                ..
            } => subquery.span().union_opt(&alias(table_alias)),
            TableFactor::TableFunction {
                expr,
                alias: table_alias,
            } => expr.span().union_opt(&alias(table_alias)),
            TableFactor::UNNEST {
                alias: table_alias,
                array_exprs,
                with_offset_alias,
                ..
            } => union(
                alias(table_alias)
                    .into_iter()
                    .chain(array_exprs.iter().map(Spanned::span))
                    .chain(with_offset_alias.as_ref().map(|i| i.span)),
            ),
            TableFactor::NestedJoin {
                table_with_joins,
                alias: table_alias,
            } => table_with_joins.span().union_opt(&alias(table_alias)),
            TableFactor::Function {
                name,
                args,
                alias: table_alias,
                ..
            } => union(
                name.0
                    .iter()
                    .map(Spanned::span)
                    .chain(args.iter().map(Spanned::span))
                    .chain(alias(table_alias)),
            ),
            TableFactor::JsonTable { .. } | TableFactor::OpenJsonTable { .. } => Span::empty(),
            TableFactor::Pivot {
                table,
                aggregate_functions,
                value_column,
                value_source,
                default_on_null,
                alias: table_alias,
            } => union(
                std::iter::once(table.span())
                    .chain(aggregate_functions.iter().map(Spanned::span))
                    .chain(value_column.iter().map(Spanned::span))
                    .chain(std::iter::once(value_source.span()))
                    .chain(default_on_null.as_ref().map(Spanned::span))
                    .chain(alias(table_alias)),
            ),
            TableFactor::Unpivot {
                table,
                value,
                name,
                columns,
                alias: table_alias,
            } => union(
                [table.span(), value.span(), name.span]
                    .into_iter()
                    .chain(columns.iter().map(Spanned::span))
                    .chain(alias(table_alias)),
            ),
            TableFactor::Other(other) => other.span,
        }
    }
}

impl Spanned for PivotValueSource {
    fn span(&self) -> Span {
        match self {
            PivotValueSource::List(items) => union(items.iter().map(Spanned::span)),
            PivotValueSource::Any(items) => union(items.iter().map(Spanned::span)),
            PivotValueSource::Subquery(query) => query.span(),
        }
    }
}

impl Spanned for ExprWithAlias {
    fn span(&self) -> Span {
        self.expr
            .span()
            .union_opt(&self.alias.as_ref().map(|i| i.span))
    }
}

impl Spanned for Expr {
    fn span(&self) -> Span {
        match self {
            Expr::Identifier(ident) => ident.span,
            Expr::CompoundIdentifier(idents) => union(idents.iter().map(|i| i.span)),
            Expr::CompoundFieldAccess { root, access_chain } => {
                union(std::iter::once(root.span()).chain(access_chain.iter().map(Spanned::span)))
            }
            Expr::IsFalse(e)
            | Expr::IsNotFalse(e)
            | Expr::IsTrue(e)
            | Expr::IsNotTrue(e)
            | Expr::IsNull(e)
            | Expr::IsNotNull(e)
            | Expr::IsUnknown(e)
            | Expr::IsNotUnknown(e)
            | Expr::Ceil { expr: e, .. }
            | Expr::Floor { expr: e, .. }
            | Expr::Nested(e)
            | Expr::UnaryOp { expr: e, .. }
            | Expr::Cast { expr: e, .. }
            | Expr::Extract { expr: e, .. } => e.span(),
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => a.span().union(&b.span()),
            Expr::InList { expr, list, .. } => {
                union(std::iter::once(expr.span()).chain(list.iter().map(Spanned::span)))
            }
            Expr::InSubquery { expr, subquery, .. } => expr.span().union(&subquery.span()),
            Expr::Between {
                expr, low, high, ..
            } => expr.span().union(&low.span()).union(&high.span()),
            Expr::BinaryOp { left, right, .. }
            | Expr::AnyOp { left, right, .. }
            | Expr::AllOp { left, right, .. } => left.span().union(&right.span()),
            Expr::Like { expr, pattern, .. }
            | Expr::ILike { expr, pattern, .. }
            | Expr::SimilarTo { expr, pattern, .. } => expr.span().union(&pattern.span()),
            Expr::RLike { .. } => Span::empty(),
            Expr::Position { expr, r#in } => expr.span().union(&r#in.span()),
            Expr::Overlay {
                expr,
                overlay_what,
                overlay_from,
                overlay_for,
            } => expr
                .span()
                .union(&overlay_what.span())
                .union(&overlay_from.span())
                .union_opt(&overlay_for.as_ref().map(|e| e.span())),
            Expr::Collate { expr, collation } => expr
                .span()
                .union(&union(collation.0.iter().map(Spanned::span))),
            Expr::Value(value) => value.span,
            Expr::TypedString(typed) => typed.value.span,
            Expr::Function(function) => function.span(),
            Expr::GroupingSets(sets) | Expr::Cube(sets) | Expr::Rollup(sets) => {
                union(sets.iter().flatten().map(Spanned::span))
            }
            Expr::Tuple(exprs) => union(exprs.iter().map(Spanned::span)),
            Expr::Array(array) => array.span(),
            Expr::JsonAccess { value, path } => value.span().union(&path.span()),
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => timestamp.span().union(&time_zone.span()),
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => union(
                std::iter::once(expr.span())
                    .chain(substring_from.as_ref().map(|e| e.span()))
                    .chain(substring_for.as_ref().map(|e| e.span())),
            ),
            Expr::Trim {
                expr,
                trim_what,
                trim_characters,
                ..
            } => union(
                std::iter::once(expr.span())
                    .chain(trim_what.as_ref().map(|e| e.span()))
                    .chain(
                        trim_characters
                            .as_ref()
                            .map(|items| union(items.iter().map(Spanned::span))),
                    ),
            ),
            Expr::Case {
                case_token,
                end_token,
                operand,
                conditions,
                else_result,
            } => union(
                std::iter::once(case_token.0.span)
                    .chain(operand.as_ref().map(|e| e.span()))
                    .chain(
                        conditions
                            .iter()
                            .flat_map(|when| [when.condition.span(), when.result.span()]),
                    )
                    .chain(else_result.as_ref().map(|e| e.span()))
                    .chain(std::iter::once(end_token.0.span)),
            ),
            Expr::Exists { subquery, .. } => subquery.span(),
            Expr::Subquery(query) => query.span(),
            Expr::Interval(interval) => interval.value.span(),
            Expr::Wildcard(token) => token.0.span,
            Expr::QualifiedWildcard(name, token) => union(
                name.0
                    .iter()
                    .map(Spanned::span)
                    .chain(std::iter::once(token.0.span)),
            ),
            Expr::Other(other) => other.span,
        }
    }
}

impl Spanned for Subscript {
    fn span(&self) -> Span {
        match self {
            Subscript::Index { index } => index.span(),
            Subscript::Slice {
                lower_bound,
                upper_bound,
                stride,
            } => union(
                [lower_bound, upper_bound, stride]
                    .into_iter()
                    .flatten()
                    .map(Spanned::span),
            ),
        }
    }
}

impl Spanned for AccessExpr {
    fn span(&self) -> Span {
        match self {
            AccessExpr::Dot(expr) => expr.span(),
            AccessExpr::Subscript(subscript) => subscript.span(),
        }
    }
}

impl Spanned for JsonPath {
    fn span(&self) -> Span {
        union(self.path.iter().map(|elem| match elem {
            JsonPathElem::Dot { .. } => Span::empty(),
            JsonPathElem::Bracket { key } | JsonPathElem::ColonBracket { key } => key.span(),
        }))
    }
}

impl Spanned for Array {
    fn span(&self) -> Span {
        union(self.elem.iter().map(Spanned::span))
    }
}

impl Spanned for Function {
    fn span(&self) -> Span {
        union(
            self.name
                .0
                .iter()
                .map(Spanned::span)
                .chain([self.args.span(), self.parameters.span()])
                .chain(self.filter.iter().map(|e| e.span()))
                .chain(self.within_group.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for FunctionArguments {
    fn span(&self) -> Span {
        match self {
            FunctionArguments::None => Span::empty(),
            FunctionArguments::Subquery(query) => query.span(),
            FunctionArguments::List(list) => list.span(),
        }
    }
}

impl Spanned for FunctionArgumentList {
    fn span(&self) -> Span {
        union(
            self.args
                .iter()
                .map(Spanned::span)
                .chain(self.clauses.iter().map(Spanned::span)),
        )
    }
}

impl Spanned for FunctionArgumentClause {
    fn span(&self) -> Span {
        match self {
            FunctionArgumentClause::IgnoreOrRespectNulls(_) => Span::empty(),
            FunctionArgumentClause::OrderBy(order_by) => {
                union(order_by.iter().map(|o| o.expr.span()))
            }
            FunctionArgumentClause::Limit(expr) => expr.span(),
            FunctionArgumentClause::Separator(value) => value.span,
            FunctionArgumentClause::Other(_, exprs) => union(exprs.iter().map(Spanned::span)),
        }
    }
}

impl Spanned for FunctionArg {
    fn span(&self) -> Span {
        match self {
            FunctionArg::Named { name, arg, .. } => name.span.union(&arg.span()),
            FunctionArg::ExprNamed { name, arg, .. } => name.span().union(&arg.span()),
            FunctionArg::Unnamed(arg) => arg.span(),
        }
    }
}

impl Spanned for FunctionArgExpr {
    fn span(&self) -> Span {
        match self {
            FunctionArgExpr::Expr(expr) => expr.span(),
            FunctionArgExpr::QualifiedWildcard(name) => name.span(),
            FunctionArgExpr::Wildcard => Span::empty(),
        }
    }
}
