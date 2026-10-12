//! Queries.

use crate::{
    AttachedToken, DataType, Expr, FunctionArg, Ident, JsonPath, ObjectName, Parens, Span,
    Statement, ValueWithSpan,
};

/// A query: `[WITH ...] body [ORDER BY ...] [LIMIT ...]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Query {
    pub with: Option<With>,
    pub body: Box<SetExpr>,
    pub order_by: Option<OrderBy>,
    pub limit_clause: Option<LimitClause>,
    pub fetch: Option<Fetch>,
    /// `FOR UPDATE` and the like: the tables they name
    pub locks: Vec<LockClause>,
    /// The expressions of the clauses the analyzer doesn't model (`SETTINGS`, pipe
    /// operators, ...)
    pub other: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LockClause {
    /// The text of the clause (`FOR UPDATE OF t NOWAIT`)
    pub text: String,
    pub of: Option<ObjectName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct With {
    pub with_token: AttachedToken,
    pub recursive: bool,
    pub cte_tables: Vec<Cte>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cte {
    pub alias: TableAlias,
    pub query: Box<Query>,
    pub from: Option<Ident>,
    pub materialized: Option<CteAsMaterialized>,
    pub closing_paren_token: AttachedToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CteAsMaterialized {
    Materialized,
    NotMaterialized,
}

/// The body of a query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SetExpr {
    Select(Box<Select>),
    /// A parenthesized query
    Query(Box<Query>),
    SetOperation {
        left: Box<SetExpr>,
        op: SetOperator,
        set_quantifier: SetQuantifier,
        right: Box<SetExpr>,
    },
    Values(Values),
    Insert(Statement),
    Update(Statement),
    Delete(Statement),
    Merge(Statement),
    /// `TABLE t`
    Table(Box<Table>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetOperator {
    Union,
    Except,
    Intersect,
    Minus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetQuantifier {
    All,
    Distinct,
    ByName,
    AllByName,
    DistinctByName,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Table {
    pub table_name: Option<String>,
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Values {
    pub explicit_row: bool,
    pub value_keyword: bool,
    pub rows: Vec<Parens<Vec<Expr>>>,
}

/// A `SELECT` block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Select {
    pub select_token: AttachedToken,
    pub distinct: Option<Distinct>,
    /// `TOP n` (T-SQL): its quantity
    pub top: Option<Expr>,
    pub projection: Vec<SelectItem>,
    /// `SELECT * EXCLUDE ...` after the projection (Snowflake)
    pub exclude: Option<ExcludeSelectItem>,
    pub into: Option<SelectInto>,
    pub from: Vec<TableWithJoins>,
    pub lateral_views: Vec<LateralView>,
    pub prewhere: Option<Expr>,
    pub selection: Option<Expr>,
    /// `START WITH` / `CONNECT BY` expressions
    pub connect_by: Vec<Expr>,
    pub group_by: GroupByExpr,
    pub cluster_by: Vec<Expr>,
    pub distribute_by: Vec<Expr>,
    pub sort_by: Vec<OrderByExpr>,
    pub having: Option<Expr>,
    pub named_window: Vec<NamedWindowDefinition>,
    pub qualify: Option<Expr>,
    pub window_before_qualify: bool,
    /// `SELECT AS STRUCT` / `AS VALUE` (BigQuery): its text
    pub value_table_mode: Option<String>,
    /// `FROM t SELECT ...` (DuckDB)
    pub from_first: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Distinct {
    All,
    Distinct,
    On(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectInto {
    pub temporary: bool,
    pub unlogged: bool,
    pub table: bool,
    pub targets: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LateralView {
    pub lateral_view: Expr,
    pub lateral_view_name: ObjectName,
    pub lateral_col_alias: Vec<Ident>,
    pub outer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SelectItem {
    UnnamedExpr(Expr),
    ExprWithAlias { expr: Expr, alias: Ident },
    ExprWithAliases { expr: Expr, aliases: Vec<Ident> },
    QualifiedWildcard(SelectItemQualifiedWildcardKind, WildcardAdditionalOptions),
    Wildcard(WildcardAdditionalOptions),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SelectItemQualifiedWildcardKind {
    ObjectName(ObjectName),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct WildcardAdditionalOptions {
    pub wildcard_token: AttachedToken,
    pub opt_ilike: Option<IlikeSelectItem>,
    pub opt_exclude: Option<ExcludeSelectItem>,
    pub opt_except: Option<ExceptSelectItem>,
    pub opt_replace: Option<ReplaceSelectItem>,
    pub opt_rename: Option<RenameSelectItem>,
    pub opt_alias: Option<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IlikeSelectItem {
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExcludeSelectItem {
    Single(ObjectName),
    Multiple(Vec<ObjectName>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExceptSelectItem {
    pub first_element: Ident,
    pub additional_elements: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplaceSelectItem {
    pub items: Vec<Box<ReplaceSelectElement>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplaceSelectElement {
    pub expr: Expr,
    pub column_name: Ident,
    pub as_keyword: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenameSelectItem {
    Single(crate::IdentWithAlias),
    Multiple(Vec<crate::IdentWithAlias>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableWithJoins {
    pub relation: TableFactor,
    pub joins: Vec<Join>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Join {
    pub relation: TableFactor,
    pub global: bool,
    pub join_operator: JoinOperator,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JoinOperator {
    Join(JoinConstraint),
    Inner(JoinConstraint),
    Left(JoinConstraint),
    LeftOuter(JoinConstraint),
    Right(JoinConstraint),
    RightOuter(JoinConstraint),
    FullOuter(JoinConstraint),
    CrossJoin(JoinConstraint),
    Semi(JoinConstraint),
    LeftSemi(JoinConstraint),
    RightSemi(JoinConstraint),
    Anti(JoinConstraint),
    LeftAnti(JoinConstraint),
    RightAnti(JoinConstraint),
    CrossApply,
    OuterApply,
    AsOf {
        match_condition: Expr,
        constraint: JoinConstraint,
    },
    StraightJoin(JoinConstraint),
    ArrayJoin,
    LeftArrayJoin,
    InnerArrayJoin,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JoinConstraint {
    On(Expr),
    Using(Vec<ObjectName>),
    Natural,
    None,
}

/// An item of a FROM clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TableFactor {
    Table {
        name: ObjectName,
        alias: Option<TableAlias>,
        /// Table function arguments (`FROM generate_series(1, 3)`)
        args: Option<TableFunctionArgs>,
        with_hints: Vec<Expr>,
        /// `FOR SYSTEM_TIME AS OF ...` and the like: its expressions
        version: Vec<Expr>,
        with_ordinality: bool,
        partitions: Vec<Ident>,
        json_path: Option<JsonPath>,
        /// `TABLESAMPLE ...`: its expressions
        sample: Vec<Expr>,
    },
    Derived {
        lateral: bool,
        subquery: Box<Query>,
        alias: Option<TableAlias>,
    },
    TableFunction {
        expr: Expr,
        alias: Option<TableAlias>,
    },
    Function {
        lateral: bool,
        name: ObjectName,
        args: Vec<FunctionArg>,
        with_ordinality: bool,
        alias: Option<TableAlias>,
    },
    UNNEST {
        alias: Option<TableAlias>,
        array_exprs: Vec<Expr>,
        with_offset: bool,
        with_offset_alias: Option<Ident>,
        with_ordinality: bool,
    },
    JsonTable {
        json_expr: Expr,
        json_path: ValueWithSpan,
        columns: Vec<JsonTableColumn>,
        alias: Option<TableAlias>,
    },
    OpenJsonTable {
        json_expr: Expr,
        json_path: Option<ValueWithSpan>,
        alias: Option<TableAlias>,
    },
    NestedJoin {
        table_with_joins: Box<TableWithJoins>,
        alias: Option<TableAlias>,
    },
    Pivot {
        table: Box<TableFactor>,
        aggregate_functions: Vec<ExprWithAlias>,
        value_column: Vec<Expr>,
        value_source: PivotValueSource,
        default_on_null: Option<Expr>,
        alias: Option<TableAlias>,
    },
    Unpivot {
        table: Box<TableFactor>,
        value: Expr,
        name: Ident,
        columns: Vec<ExprWithAlias>,
        alias: Option<TableAlias>,
    },
    /// A FROM item the analyzer doesn't model (`XMLTABLE`, `MATCH_RECOGNIZE`, ...)
    Other(OtherTableFactor),
}

/// A FROM item the analyzer doesn't model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OtherTableFactor {
    pub text: String,
    pub span: Span,
    pub alias: Option<TableAlias>,
    pub tables: Vec<TableFactor>,
    pub exprs: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableFunctionArgs {
    pub args: Vec<FunctionArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableAlias {
    pub explicit: bool,
    pub name: Ident,
    pub columns: Vec<TableAliasColumnDef>,
    pub at: Option<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableAliasColumnDef {
    pub name: Ident,
    pub data_type: Option<DataType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExprWithAlias {
    pub expr: Expr,
    pub alias: Option<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PivotValueSource {
    List(Vec<ExprWithAlias>),
    Any(Vec<OrderByExpr>),
    Subquery(Box<Query>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsonTableColumn {
    Named(JsonTableNamedColumn),
    ForOrdinality(Ident),
    Nested(JsonTableNestedColumn),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsonTableNamedColumn {
    pub name: Ident,
    pub r#type: DataType,
    pub path: ValueWithSpan,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsonTableNestedColumn {
    pub path: ValueWithSpan,
    pub columns: Vec<JsonTableColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupByExpr {
    All(Vec<GroupByWithModifier>),
    Expressions(Vec<Expr>, Vec<GroupByWithModifier>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupByWithModifier {
    Rollup,
    Cube,
    Totals,
    GroupingSets(Expr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NamedWindowDefinition(pub Ident, pub NamedWindowExpr);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NamedWindowExpr {
    NamedWindow(Ident),
    WindowSpec(crate::WindowSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderBy {
    pub kind: OrderByKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrderByKind {
    All(OrderByOptions),
    Expressions(Vec<OrderByExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderByExpr {
    pub expr: Expr,
    pub options: OrderByOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct OrderByOptions {
    pub sort: Option<OrderBySort>,
    pub nulls_first: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrderBySort {
    Asc,
    Desc,
    Using(ObjectName),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LimitClause {
    LimitOffset {
        limit: Option<Expr>,
        offset: Option<Offset>,
        limit_by: Vec<Expr>,
    },
    /// MySQL `LIMIT offset, limit`
    OffsetCommaLimit { offset: Expr, limit: Expr },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Offset {
    pub value: Expr,
    pub rows: OffsetRows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OffsetRows {
    None,
    Row,
    Rows,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fetch {
    pub with_ties: bool,
    pub percent: bool,
    pub quantity: Option<Expr>,
}
