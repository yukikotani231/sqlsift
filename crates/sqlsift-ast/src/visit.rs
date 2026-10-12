//! Walks over the tree (sqlparser's `Visit` / `Visitor`): every node's children are
//! visited in the order of its fields; queries, expressions, statements, FROM items
//! and the relations they name are reported to the visitor before and after their
//! children.

use std::ops::ControlFlow;

use crate::{
    AccessExpr, AlterColumnOperation, AlterTable, AlterTableOperation, Assignment,
    AssignmentTarget, CaseWhen, CheckConstraint, ColumnDef, ColumnOption, ColumnOptionDef,
    CreateIndex, CreateTable, CreateTrigger, CreateView, Cte, Declare, Delete, Distinct, DoUpdate,
    Expr, ExprWithAlias, Fetch, FromTable, Function, FunctionArg, FunctionArgExpr,
    FunctionArgumentClause, FunctionArguments, GroupByExpr, GroupByWithModifier, IndexColumn,
    Insert, Join, JoinConstraint, JoinOperator, JsonPath, JsonPathElem, JsonTableColumn,
    LateralView, LimitClause, Merge, NamedWindowDefinition, NamedWindowExpr, ObjectName,
    ObjectNamePart, Offset, OnConflict, OnConflictAction, OnInsert, OrderBy, OrderByExpr,
    OrderByKind, PivotValueSource, Query, ReplaceSelectItem, Select, SelectInto, SelectItem,
    SelectItemQualifiedWildcardKind, Set, SetExpr, Statement, Subscript, TableConstraint,
    TableFactor, TableFunctionArgs, TableObject, TableWithJoins, Update, UpdateTableFromKind,
    Values, WildcardAdditionalOptions, WindowFrameBound, WindowSpec, WindowType, With,
};

/// Called on the nodes of a tree as [`Visit::visit`] walks it.
pub trait Visitor {
    /// What stops the walk
    type Break;

    fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self, _expr: &Expr) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn post_visit_expr(&mut self, _expr: &Expr) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn pre_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn post_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn pre_visit_table_factor(&mut self, _factor: &TableFactor) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn post_visit_table_factor(&mut self, _factor: &TableFactor) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    /// A table or view a statement names (FROM items, the targets of INSERT, UPDATE,
    /// DELETE, the names of created and altered tables)
    fn pre_visit_relation(&mut self, _relation: &ObjectName) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
    fn post_visit_relation(&mut self, _relation: &ObjectName) -> ControlFlow<Self::Break> {
        ControlFlow::Continue(())
    }
}

/// A node that can be walked.
pub trait Visit {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break>;
}

impl<T: Visit> Visit for Vec<T> {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        for item in self {
            item.visit(visitor)?;
        }
        ControlFlow::Continue(())
    }
}

impl<T: Visit> Visit for Option<T> {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        if let Some(item) = self {
            item.visit(visitor)?;
        }
        ControlFlow::Continue(())
    }
}

impl<T: Visit> Visit for Box<T> {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        T::visit(self, visitor)
    }
}

impl<T: Visit> Visit for crate::Parens<T> {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        self.content.visit(visitor)
    }
}

fn relation<V: Visitor>(name: &ObjectName, visitor: &mut V) -> ControlFlow<V::Break> {
    visitor.pre_visit_relation(name)?;
    name.visit(visitor)?;
    visitor.post_visit_relation(name)
}

/// Implements [`Visit`] by visiting the listed fields of a struct in order.
macro_rules! visit_fields {
    ($($ty:ty => [$($field:tt),*];)*) => {$(
        impl Visit for $ty {
            #[allow(unused_variables)]
            fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
                $(self.$field.visit(visitor)?;)*
                ControlFlow::Continue(())
            }
        }
    )*};
}

visit_fields! {
    With => [cte_tables];
    Cte => [query];
    Select => [distinct, top, projection, into, from, lateral_views, prewhere, selection,
        connect_by, group_by, cluster_by, distribute_by, sort_by, having, named_window, qualify];
    SelectInto => [targets];
    LateralView => [lateral_view];
    TableWithJoins => [relation, joins];
    Join => [relation, join_operator];
    OrderBy => [kind];
    OrderByExpr => [expr];
    Offset => [value];
    Fetch => [quantity];
    Values => [rows];
    ExprWithAlias => [expr];
    TableFunctionArgs => [args];
    Function => [name, parameters, args, filter, over, within_group];
    WindowSpec => [partition_by, order_by, window_frame];
    CaseWhen => [condition, result];
    ReplaceSelectItem => [items];
    JsonPath => [path];
    Assignment => [target, value];
    OnConflict => [action];
    DoUpdate => [assignments, selection];
    Update => [table, assignments, from, selection, returning, order_by, limit];
    Delete => [tables, from, using, selection, returning, order_by, limit];
    Merge => [table, source, on, clauses];
    CreateView => [query];
    CreateIndex => [columns, predicate];
    IndexColumn => [column];
    ColumnDef => [options];
    ColumnOptionDef => [option];
    CheckConstraint => [expr];
    CreateTrigger => [condition];
    Declare => [assignment, for_query];
    NamedWindowDefinition => [1];
}

impl Visit for crate::Ident {
    fn visit<V: Visitor>(&self, _visitor: &mut V) -> ControlFlow<V::Break> {
        ControlFlow::Continue(())
    }
}

impl Visit for ObjectName {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        self.0.visit(visitor)
    }
}

impl Visit for ObjectNamePart {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            ObjectNamePart::Identifier(_) => ControlFlow::Continue(()),
            ObjectNamePart::Function(function) => function.args.visit(visitor),
        }
    }
}

impl Visit for Statement {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        visitor.pre_visit_statement(self)?;
        match self {
            Statement::Query(query) => query.visit(visitor)?,
            Statement::Insert(insert) => insert.visit(visitor)?,
            Statement::Update(update) => update.visit(visitor)?,
            Statement::Delete(delete) => delete.visit(visitor)?,
            Statement::Merge(merge) => merge.visit(visitor)?,
            Statement::CreateTable(create) => create.visit(visitor)?,
            Statement::CreateView(view) => {
                relation(&view.name, visitor)?;
                view.visit(visitor)?;
            }
            Statement::CreateIndex(index) => {
                relation(&index.table_name, visitor)?;
                index.visit(visitor)?;
            }
            Statement::CreateTrigger(trigger) => trigger.visit(visitor)?,
            Statement::AlterTable(alter) => alter.visit(visitor)?,
            Statement::Set(Set::SingleAssignment { values, .. }) => values.visit(visitor)?,
            Statement::Declare { stmts } => stmts.visit(visitor)?,
            Statement::Other(other) => {
                for name in &other.relations {
                    relation(name, visitor)?;
                }
                other.exprs.visit(visitor)?;
                other.queries.visit(visitor)?;
            }
            Statement::CreateType { .. }
            | Statement::AlterType(_)
            | Statement::Drop { .. }
            | Statement::Set(Set::Other)
            | Statement::Use(_) => {}
        }
        visitor.post_visit_statement(self)
    }
}

impl Visit for Insert {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match &self.table {
            TableObject::TableName(name) => relation(name, visitor)?,
            TableObject::TableFunction(function) => function.visit(visitor)?,
            TableObject::TableQuery(query) => query.visit(visitor)?,
        }
        self.columns.visit(visitor)?;
        self.source.visit(visitor)?;
        self.assignments.visit(visitor)?;
        self.partitioned.visit(visitor)?;
        self.on.visit(visitor)?;
        self.returning.visit(visitor)
    }
}

impl Visit for OnInsert {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            OnInsert::DuplicateKeyUpdate(assignments) => assignments.visit(visitor),
            OnInsert::OnConflict(on_conflict) => on_conflict.visit(visitor),
        }
    }
}

impl Visit for OnConflictAction {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            OnConflictAction::DoNothing => ControlFlow::Continue(()),
            OnConflictAction::DoUpdate(update) => update.visit(visitor),
        }
    }
}

impl Visit for AssignmentTarget {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            AssignmentTarget::ColumnName(name) => name.visit(visitor),
            AssignmentTarget::Tuple(names) => names.visit(visitor),
        }
    }
}

impl Visit for UpdateTableFromKind {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            UpdateTableFromKind::BeforeSet(from) | UpdateTableFromKind::AfterSet(from) => {
                from.visit(visitor)
            }
        }
    }
}

impl Visit for FromTable {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => {
                from.visit(visitor)
            }
        }
    }
}

impl Visit for CreateTable {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        relation(&self.name, visitor)?;
        self.columns.visit(visitor)?;
        self.constraints.visit(visitor)?;
        self.query.visit(visitor)?;
        self.other.visit(visitor)
    }
}

impl Visit for ColumnOption {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            ColumnOption::Default(expr) | ColumnOption::OnUpdate(expr) => expr.visit(visitor),
            ColumnOption::Check(check) => check.visit(visitor),
            ColumnOption::Generated {
                generation_expr, ..
            } => generation_expr.visit(visitor),
            ColumnOption::Other(_, exprs) => exprs.visit(visitor),
            ColumnOption::ForeignKey(fk) => relation(&fk.foreign_table, visitor),
            ColumnOption::Null
            | ColumnOption::NotNull
            | ColumnOption::PrimaryKey(_)
            | ColumnOption::Unique(_)
            | ColumnOption::DialectSpecific(_)
            | ColumnOption::Identity => ControlFlow::Continue(()),
        }
    }
}

impl Visit for TableConstraint {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            TableConstraint::Check(check) => check.visit(visitor),
            TableConstraint::ForeignKey(fk) => relation(&fk.foreign_table, visitor),
            TableConstraint::Unique(unique) => unique.columns.visit(visitor),
            TableConstraint::PrimaryKey(key) => key.columns.visit(visitor),
            TableConstraint::Index(index) | TableConstraint::FulltextOrSpatial(index) => {
                index.columns.visit(visitor)
            }
            TableConstraint::Exclude(exclude) => {
                exclude.elements.visit(visitor)?;
                exclude.predicate.visit(visitor)
            }
            TableConstraint::PrimaryKeyUsingIndex(_) | TableConstraint::UniqueUsingIndex(_) => {
                ControlFlow::Continue(())
            }
        }
    }
}

impl Visit for AlterTable {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        relation(&self.name, visitor)?;
        self.operations.visit(visitor)
    }
}

impl Visit for AlterTableOperation {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            AlterTableOperation::AddConstraint { constraint, .. } => constraint.visit(visitor),
            AlterTableOperation::AddColumn { column_def, .. } => column_def.visit(visitor),
            AlterTableOperation::ChangeColumn { options, .. }
            | AlterTableOperation::ModifyColumn { options, .. } => options.visit(visitor),
            AlterTableOperation::AlterColumn { op, .. } => match op {
                AlterColumnOperation::SetDefault { value } => value.visit(visitor),
                AlterColumnOperation::SetDataType { using, .. } => using.visit(visitor),
                _ => ControlFlow::Continue(()),
            },
            AlterTableOperation::RenameTable { .. } => ControlFlow::Continue(()),
            AlterTableOperation::Other(_, exprs) => exprs.visit(visitor),
            AlterTableOperation::DropConstraint { .. }
            | AlterTableOperation::DropColumn { .. }
            | AlterTableOperation::DropPrimaryKey
            | AlterTableOperation::RenameColumn { .. }
            | AlterTableOperation::RenameConstraint { .. } => ControlFlow::Continue(()),
        }
    }
}

impl Visit for Query {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        visitor.pre_visit_query(self)?;
        self.with.visit(visitor)?;
        self.body.visit(visitor)?;
        self.order_by.visit(visitor)?;
        self.limit_clause.visit(visitor)?;
        self.fetch.visit(visitor)?;
        self.other.visit(visitor)?;
        visitor.post_visit_query(self)
    }
}

impl Visit for SetExpr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            SetExpr::Select(select) => select.visit(visitor),
            SetExpr::Query(query) => query.visit(visitor),
            SetExpr::SetOperation { left, right, .. } => {
                left.visit(visitor)?;
                right.visit(visitor)
            }
            SetExpr::Values(values) => values.visit(visitor),
            SetExpr::Insert(statement)
            | SetExpr::Update(statement)
            | SetExpr::Delete(statement)
            | SetExpr::Merge(statement) => statement.visit(visitor),
            SetExpr::Table(_) => ControlFlow::Continue(()),
        }
    }
}

impl Visit for Distinct {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            Distinct::On(exprs) => exprs.visit(visitor),
            Distinct::All | Distinct::Distinct => ControlFlow::Continue(()),
        }
    }
}

impl Visit for SelectItem {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            SelectItem::UnnamedExpr(expr)
            | SelectItem::ExprWithAlias { expr, .. }
            | SelectItem::ExprWithAliases { expr, .. } => expr.visit(visitor),
            SelectItem::QualifiedWildcard(kind, options) => {
                if let SelectItemQualifiedWildcardKind::Expr(expr) = kind {
                    expr.visit(visitor)?;
                }
                options.visit(visitor)
            }
            SelectItem::Wildcard(options) => options.visit(visitor),
        }
    }
}

impl Visit for WildcardAdditionalOptions {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        self.opt_replace.visit(visitor)
    }
}

impl Visit for crate::ReplaceSelectElement {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        self.expr.visit(visitor)
    }
}

impl Visit for GroupByExpr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            GroupByExpr::All(modifiers) => modifiers.visit(visitor),
            GroupByExpr::Expressions(exprs, modifiers) => {
                exprs.visit(visitor)?;
                modifiers.visit(visitor)
            }
        }
    }
}

impl Visit for GroupByWithModifier {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            GroupByWithModifier::GroupingSets(expr) => expr.visit(visitor),
            _ => ControlFlow::Continue(()),
        }
    }
}

impl Visit for NamedWindowExpr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            NamedWindowExpr::NamedWindow(_) => ControlFlow::Continue(()),
            NamedWindowExpr::WindowSpec(spec) => spec.visit(visitor),
        }
    }
}

impl Visit for TableFactor {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        visitor.pre_visit_table_factor(self)?;
        match self {
            TableFactor::Table {
                name,
                args,
                with_hints,
                version,
                sample,
                ..
            } => {
                relation(name, visitor)?;
                if let Some(args) = args {
                    args.args.visit(visitor)?;
                }
                with_hints.visit(visitor)?;
                version.visit(visitor)?;
                sample.visit(visitor)?;
            }
            TableFactor::Derived { subquery, .. } => subquery.visit(visitor)?,
            TableFactor::TableFunction { expr, .. } => expr.visit(visitor)?,
            TableFactor::Function { name, args, .. } => {
                name.visit(visitor)?;
                args.visit(visitor)?;
            }
            TableFactor::UNNEST { array_exprs, .. } => array_exprs.visit(visitor)?,
            TableFactor::JsonTable {
                json_expr, columns, ..
            } => {
                json_expr.visit(visitor)?;
                columns.visit(visitor)?;
            }
            TableFactor::OpenJsonTable { json_expr, .. } => json_expr.visit(visitor)?,
            TableFactor::NestedJoin {
                table_with_joins, ..
            } => table_with_joins.visit(visitor)?,
            TableFactor::Pivot {
                table,
                aggregate_functions,
                value_column,
                value_source,
                default_on_null,
                ..
            } => {
                table.visit(visitor)?;
                aggregate_functions.visit(visitor)?;
                value_column.visit(visitor)?;
                value_source.visit(visitor)?;
                default_on_null.visit(visitor)?;
            }
            TableFactor::Unpivot {
                table,
                value,
                columns,
                ..
            } => {
                table.visit(visitor)?;
                value.visit(visitor)?;
                columns.visit(visitor)?;
            }
            TableFactor::Other(other) => {
                other.tables.visit(visitor)?;
                other.exprs.visit(visitor)?;
            }
        }
        visitor.post_visit_table_factor(self)
    }
}

impl Visit for JsonTableColumn {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            JsonTableColumn::Nested(nested) => nested.columns.visit(visitor),
            JsonTableColumn::Named(_) | JsonTableColumn::ForOrdinality(_) => {
                ControlFlow::Continue(())
            }
        }
    }
}

impl Visit for PivotValueSource {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            PivotValueSource::List(items) => items.visit(visitor),
            PivotValueSource::Any(items) => items.visit(visitor),
            PivotValueSource::Subquery(query) => query.visit(visitor),
        }
    }
}

impl Visit for JoinOperator {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
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
            | JoinOperator::StraightJoin(c) => c.visit(visitor),
            JoinOperator::AsOf {
                match_condition,
                constraint,
            } => {
                match_condition.visit(visitor)?;
                constraint.visit(visitor)
            }
            JoinOperator::CrossApply
            | JoinOperator::OuterApply
            | JoinOperator::ArrayJoin
            | JoinOperator::LeftArrayJoin
            | JoinOperator::InnerArrayJoin => ControlFlow::Continue(()),
        }
    }
}

impl Visit for JoinConstraint {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            JoinConstraint::On(expr) => expr.visit(visitor),
            JoinConstraint::Using(names) => names.visit(visitor),
            JoinConstraint::Natural | JoinConstraint::None => ControlFlow::Continue(()),
        }
    }
}

impl Visit for OrderByKind {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            OrderByKind::All(_) => ControlFlow::Continue(()),
            OrderByKind::Expressions(exprs) => exprs.visit(visitor),
        }
    }
}

impl Visit for LimitClause {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            } => {
                limit.visit(visitor)?;
                offset.visit(visitor)?;
                limit_by.visit(visitor)
            }
            LimitClause::OffsetCommaLimit { offset, limit } => {
                offset.visit(visitor)?;
                limit.visit(visitor)
            }
        }
    }
}

impl Visit for Expr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        visitor.pre_visit_expr(self)?;
        match self {
            Expr::Identifier(_)
            | Expr::CompoundIdentifier(_)
            | Expr::Value(_)
            | Expr::TypedString(_)
            | Expr::Wildcard(_)
            | Expr::QualifiedWildcard(..) => {}
            Expr::CompoundFieldAccess { root, access_chain } => {
                root.visit(visitor)?;
                access_chain.visit(visitor)?;
            }
            Expr::JsonAccess { value, path } => {
                value.visit(visitor)?;
                path.visit(visitor)?;
            }
            Expr::IsFalse(e)
            | Expr::IsNotFalse(e)
            | Expr::IsTrue(e)
            | Expr::IsNotTrue(e)
            | Expr::IsNull(e)
            | Expr::IsNotNull(e)
            | Expr::IsUnknown(e)
            | Expr::IsNotUnknown(e)
            | Expr::Nested(e)
            | Expr::UnaryOp { expr: e, .. }
            | Expr::Cast { expr: e, .. }
            | Expr::Extract { expr: e, .. }
            | Expr::Ceil { expr: e, .. }
            | Expr::Floor { expr: e, .. }
            | Expr::Collate { expr: e, .. } => e.visit(visitor)?,
            Expr::Interval(interval) => interval.value.visit(visitor)?,
            Expr::IsDistinctFrom(a, b) | Expr::IsNotDistinctFrom(a, b) => {
                a.visit(visitor)?;
                b.visit(visitor)?;
            }
            Expr::InList { expr, list, .. } => {
                expr.visit(visitor)?;
                list.visit(visitor)?;
            }
            Expr::InSubquery { expr, subquery, .. } => {
                expr.visit(visitor)?;
                subquery.visit(visitor)?;
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                expr.visit(visitor)?;
                low.visit(visitor)?;
                high.visit(visitor)?;
            }
            Expr::BinaryOp { left, right, .. }
            | Expr::AnyOp { left, right, .. }
            | Expr::AllOp { left, right, .. } => {
                left.visit(visitor)?;
                right.visit(visitor)?;
            }
            Expr::Like {
                expr,
                pattern,
                escape_char,
                ..
            }
            | Expr::ILike {
                expr,
                pattern,
                escape_char,
                ..
            }
            | Expr::SimilarTo {
                expr,
                pattern,
                escape_char,
                ..
            } => {
                expr.visit(visitor)?;
                pattern.visit(visitor)?;
                escape_char.visit(visitor)?;
            }
            Expr::RLike { expr, pattern, .. } => {
                expr.visit(visitor)?;
                pattern.visit(visitor)?;
            }
            Expr::AtTimeZone {
                timestamp,
                time_zone,
            } => {
                timestamp.visit(visitor)?;
                time_zone.visit(visitor)?;
            }
            Expr::Position { expr, r#in } => {
                expr.visit(visitor)?;
                r#in.visit(visitor)?;
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => {
                expr.visit(visitor)?;
                substring_from.visit(visitor)?;
                substring_for.visit(visitor)?;
            }
            Expr::Trim {
                trim_what,
                expr,
                trim_characters,
                ..
            } => {
                trim_what.visit(visitor)?;
                expr.visit(visitor)?;
                trim_characters.visit(visitor)?;
            }
            Expr::Overlay {
                expr,
                overlay_what,
                overlay_from,
                overlay_for,
            } => {
                expr.visit(visitor)?;
                overlay_what.visit(visitor)?;
                overlay_from.visit(visitor)?;
                overlay_for.visit(visitor)?;
            }
            Expr::Function(function) => function.visit(visitor)?,
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                operand.visit(visitor)?;
                conditions.visit(visitor)?;
                else_result.visit(visitor)?;
            }
            Expr::Exists { subquery, .. } => subquery.visit(visitor)?,
            Expr::Subquery(query) => query.visit(visitor)?,
            Expr::GroupingSets(sets) | Expr::Cube(sets) | Expr::Rollup(sets) => {
                sets.visit(visitor)?;
            }
            Expr::Tuple(exprs) => exprs.visit(visitor)?,
            Expr::Array(array) => array.elem.visit(visitor)?,
            Expr::Other(other) => {
                other.exprs.visit(visitor)?;
                other.queries.visit(visitor)?;
            }
        }
        visitor.post_visit_expr(self)
    }
}

impl Visit for AccessExpr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            AccessExpr::Dot(expr) => expr.visit(visitor),
            AccessExpr::Subscript(subscript) => subscript.visit(visitor),
        }
    }
}

impl Visit for Subscript {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            Subscript::Index { index } => index.visit(visitor),
            Subscript::Slice {
                lower_bound,
                upper_bound,
                stride,
            } => {
                lower_bound.visit(visitor)?;
                upper_bound.visit(visitor)?;
                stride.visit(visitor)
            }
        }
    }
}

impl Visit for JsonPathElem {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            JsonPathElem::Dot { .. } => ControlFlow::Continue(()),
            JsonPathElem::Bracket { key } | JsonPathElem::ColonBracket { key } => {
                key.visit(visitor)
            }
        }
    }
}

impl Visit for FunctionArguments {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            FunctionArguments::None => ControlFlow::Continue(()),
            FunctionArguments::Subquery(query) => query.visit(visitor),
            FunctionArguments::List(list) => {
                list.args.visit(visitor)?;
                list.clauses.visit(visitor)
            }
        }
    }
}

impl Visit for FunctionArgumentClause {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            FunctionArgumentClause::OrderBy(order_by) => order_by.visit(visitor),
            FunctionArgumentClause::Limit(expr) => expr.visit(visitor),
            FunctionArgumentClause::Other(_, exprs) => exprs.visit(visitor),
            FunctionArgumentClause::IgnoreOrRespectNulls(_)
            | FunctionArgumentClause::Separator(_) => ControlFlow::Continue(()),
        }
    }
}

impl Visit for FunctionArg {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            FunctionArg::Named { arg, .. } | FunctionArg::Unnamed(arg) => arg.visit(visitor),
            FunctionArg::ExprNamed { name, arg, .. } => {
                name.visit(visitor)?;
                arg.visit(visitor)
            }
        }
    }
}

impl Visit for FunctionArgExpr {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            FunctionArgExpr::Expr(expr) => expr.visit(visitor),
            FunctionArgExpr::QualifiedWildcard(_) | FunctionArgExpr::Wildcard => {
                ControlFlow::Continue(())
            }
        }
    }
}

impl Visit for WindowType {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            WindowType::WindowSpec(spec) => spec.visit(visitor),
            WindowType::NamedWindow(_) => ControlFlow::Continue(()),
        }
    }
}

impl Visit for crate::WindowFrame {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        self.start_bound.visit(visitor)?;
        self.end_bound.visit(visitor)
    }
}

impl Visit for WindowFrameBound {
    fn visit<V: Visitor>(&self, visitor: &mut V) -> ControlFlow<V::Break> {
        match self {
            WindowFrameBound::CurrentRow => ControlFlow::Continue(()),
            WindowFrameBound::Preceding(n) | WindowFrameBound::Following(n) => n.visit(visitor),
        }
    }
}

/// Calls `f` on every relation the node names, in order.
pub fn visit_relations<N: Visit, B>(
    node: &N,
    f: impl FnMut(&ObjectName) -> ControlFlow<B>,
) -> ControlFlow<B> {
    struct Relations<F>(F);
    impl<B, F: FnMut(&ObjectName) -> ControlFlow<B>> Visitor for Relations<F> {
        type Break = B;
        fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<B> {
            (self.0)(relation)
        }
    }
    node.visit(&mut Relations(f))
}

/// Calls `f` on every expression in the node, outer expressions first.
pub fn visit_expressions<N: Visit, B>(
    node: &N,
    f: impl FnMut(&Expr) -> ControlFlow<B>,
) -> ControlFlow<B> {
    struct Expressions<F>(F);
    impl<B, F: FnMut(&Expr) -> ControlFlow<B>> Visitor for Expressions<F> {
        type Break = B;
        fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<B> {
            (self.0)(expr)
        }
    }
    node.visit(&mut Expressions(f))
}
