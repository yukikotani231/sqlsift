//! Expressions.

use crate::{AttachedToken, DataType, Ident, ObjectName, OrderByExpr, Query, Span, ValueWithSpan};

/// An expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    Identifier(Ident),
    CompoundIdentifier(Vec<Ident>),
    /// `root.field[1].x`
    CompoundFieldAccess {
        root: Box<Expr>,
        access_chain: Vec<AccessExpr>,
    },
    /// `value:path` (Snowflake, Databricks)
    JsonAccess {
        value: Box<Expr>,
        path: JsonPath,
    },
    IsFalse(Box<Expr>),
    IsNotFalse(Box<Expr>),
    IsTrue(Box<Expr>),
    IsNotTrue(Box<Expr>),
    IsNull(Box<Expr>),
    IsNotNull(Box<Expr>),
    IsUnknown(Box<Expr>),
    IsNotUnknown(Box<Expr>),
    IsDistinctFrom(Box<Expr>, Box<Expr>),
    IsNotDistinctFrom(Box<Expr>, Box<Expr>),
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<Query>,
        negated: bool,
    },
    Between {
        expr: Box<Expr>,
        negated: bool,
        low: Box<Expr>,
        high: Box<Expr>,
    },
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },
    Like {
        negated: bool,
        any: bool,
        expr: Box<Expr>,
        pattern: Box<Expr>,
        escape_char: Option<Box<Expr>>,
    },
    ILike {
        negated: bool,
        any: bool,
        expr: Box<Expr>,
        pattern: Box<Expr>,
        escape_char: Option<Box<Expr>>,
    },
    SimilarTo {
        negated: bool,
        expr: Box<Expr>,
        pattern: Box<Expr>,
        escape_char: Option<Box<Expr>>,
    },
    RLike {
        negated: bool,
        expr: Box<Expr>,
        pattern: Box<Expr>,
        regexp: bool,
    },
    AnyOp {
        left: Box<Expr>,
        compare_op: BinaryOperator,
        right: Box<Expr>,
        is_some: bool,
    },
    AllOp {
        left: Box<Expr>,
        compare_op: BinaryOperator,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOperator,
        expr: Box<Expr>,
    },
    Cast {
        kind: CastKind,
        expr: Box<Expr>,
        data_type: DataType,
        /// `FORMAT ...` (BigQuery): its text
        format: Option<String>,
    },
    AtTimeZone {
        timestamp: Box<Expr>,
        time_zone: Box<Expr>,
    },
    Extract {
        /// The field, as sqlparser prints it (`YEAR`, `EPOCH`)
        field: String,
        syntax: ExtractSyntax,
        expr: Box<Expr>,
    },
    Ceil {
        expr: Box<Expr>,
        field: CeilFloorKind,
    },
    Floor {
        expr: Box<Expr>,
        field: CeilFloorKind,
    },
    Position {
        expr: Box<Expr>,
        r#in: Box<Expr>,
    },
    Substring {
        expr: Box<Expr>,
        substring_from: Option<Box<Expr>>,
        substring_for: Option<Box<Expr>>,
        special: bool,
        shorthand: bool,
    },
    Trim {
        trim_where: Option<TrimWhereField>,
        trim_what: Option<Box<Expr>>,
        expr: Box<Expr>,
        trim_characters: Option<Vec<Expr>>,
    },
    Overlay {
        expr: Box<Expr>,
        overlay_what: Box<Expr>,
        overlay_from: Box<Expr>,
        overlay_for: Option<Box<Expr>>,
    },
    Collate {
        expr: Box<Expr>,
        collation: ObjectName,
    },
    Nested(Box<Expr>),
    Value(ValueWithSpan),
    TypedString(TypedString),
    Function(Function),
    Case {
        case_token: AttachedToken,
        end_token: AttachedToken,
        operand: Option<Box<Expr>>,
        conditions: Vec<CaseWhen>,
        else_result: Option<Box<Expr>>,
    },
    Exists {
        subquery: Box<Query>,
        negated: bool,
    },
    Subquery(Box<Query>),
    GroupingSets(Vec<Vec<Expr>>),
    Cube(Vec<Vec<Expr>>),
    Rollup(Vec<Vec<Expr>>),
    Tuple(Vec<Expr>),
    Array(Array),
    Interval(Interval),
    /// `*` as an expression
    Wildcard(AttachedToken),
    /// `t.*` as an expression
    QualifiedWildcard(ObjectName, AttachedToken),
    /// An expression the analyzer doesn't model (`CONVERT(...)`, `STRUCT(...)`, a lambda,
    /// ...): its text, its location and the expressions in it.
    Other(OtherExpr),
}

/// An expression the analyzer doesn't model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OtherExpr {
    /// Its text, as sqlparser prints it
    pub text: String,
    pub span: Span,
    /// The expressions in it, in order
    pub exprs: Vec<Expr>,
    /// The queries in it, in order
    pub queries: Vec<Query>,
}

impl Expr {
    /// A literal value with no location.
    pub fn value(value: impl Into<ValueWithSpan>) -> Self {
        Expr::Value(value.into())
    }
}

impl From<crate::Value> for ValueWithSpan {
    fn from(value: crate::Value) -> Self {
        value.with_span(Span::empty())
    }
}

/// A step of [`Expr::CompoundFieldAccess`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AccessExpr {
    /// `.field`
    Dot(Expr),
    /// `[index]`
    Subscript(Subscript),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subscript {
    Index {
        index: Expr,
    },
    Slice {
        lower_bound: Option<Expr>,
        upper_bound: Option<Expr>,
        stride: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsonPath {
    pub path: Vec<JsonPathElem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsonPathElem {
    Dot { key: String, quoted: bool },
    Bracket { key: Expr },
    ColonBracket { key: Expr },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BinaryOperator {
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
    StringConcat,
    Gt,
    Lt,
    GtEq,
    LtEq,
    Spaceship,
    Eq,
    NotEq,
    And,
    Or,
    Xor,
    BitwiseOr,
    BitwiseAnd,
    BitwiseXor,
    DuckIntegerDivide,
    MyIntegerDivide,
    Match,
    Regexp,
    Glob,
    Custom(String),
    PGBitwiseXor,
    PGBitwiseShiftLeft,
    PGBitwiseShiftRight,
    PGExp,
    PGOverlap,
    PGRegexMatch,
    PGRegexIMatch,
    PGRegexNotMatch,
    PGRegexNotIMatch,
    PGLikeMatch,
    PGILikeMatch,
    PGNotLikeMatch,
    PGNotILikeMatch,
    PGStartsWith,
    Arrow,
    LongArrow,
    HashArrow,
    HashLongArrow,
    AtAt,
    AtArrow,
    ArrowAt,
    HashMinus,
    AtQuestion,
    Question,
    QuestionAnd,
    QuestionPipe,
    PGCustomBinaryOperator(Vec<String>),
    Overlaps,
    DoubleHash,
    LtDashGt,
    AndLt,
    AndGt,
    LtLtPipe,
    PipeGtGt,
    AndLtPipe,
    PipeAndGt,
    LtCaret,
    GtCaret,
    QuestionHash,
    QuestionDash,
    QuestionDashPipe,
    QuestionDoublePipe,
    At,
    TildeEq,
    Assignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOperator {
    AtDashAt,
    BangNot,
    BitwiseNot,
    DoubleAt,
    Hash,
    Plus,
    Minus,
    Not,
    PGAbs,
    PGCubeRoot,
    PGPostfixFactorial,
    PGPrefixFactorial,
    PGSquareRoot,
    QuestionDash,
    QuestionPipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastKind {
    Cast,
    TryCast,
    SafeCast,
    DoubleColon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExtractSyntax {
    From,
    Comma,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CeilFloorKind {
    /// `CEIL(x TO field)`: the field's text
    DateTimeField(String),
    /// `CEIL(x, scale)`
    Scale(ValueWithSpan),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrimWhereField {
    Both,
    Leading,
    Trailing,
}

/// `DATE '2024-01-01'`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypedString {
    pub data_type: DataType,
    pub value: ValueWithSpan,
    pub uses_odbc_syntax: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CaseWhen {
    pub condition: Expr,
    pub result: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Array {
    pub elem: Vec<Expr>,
    /// `ARRAY[...]` rather than `[...]`
    pub named: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Interval {
    pub value: Box<Expr>,
    /// The fields and precisions after the value, as written (`DAY TO SECOND`)
    pub qualifier: Option<String>,
}

/// A function call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    pub name: ObjectName,
    pub uses_odbc_syntax: bool,
    /// ClickHouse parametric arguments (`f(0.5)(x)`)
    pub parameters: FunctionArguments,
    pub args: FunctionArguments,
    pub within_group: Vec<OrderByExpr>,
    pub filter: Option<Box<Expr>>,
    pub null_treatment: Option<NullTreatment>,
    pub over: Option<WindowType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionArguments {
    /// No parentheses (`CURRENT_TIMESTAMP`)
    None,
    Subquery(Box<Query>),
    List(FunctionArgumentList),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionArgumentList {
    pub duplicate_treatment: Option<DuplicateTreatment>,
    pub args: Vec<FunctionArg>,
    pub clauses: Vec<FunctionArgumentClause>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionArgumentClause {
    IgnoreOrRespectNulls(NullTreatment),
    OrderBy(Vec<OrderByExpr>),
    Limit(Expr),
    /// `SEPARATOR ','` (MySQL GROUP_CONCAT)
    Separator(ValueWithSpan),
    /// Any other clause: its text and expressions
    Other(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DuplicateTreatment {
    Distinct,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NullTreatment {
    IgnoreNulls,
    RespectNulls,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionArg {
    Named {
        name: Ident,
        arg: FunctionArgExpr,
        operator: FunctionArgOperator,
    },
    ExprNamed {
        name: Expr,
        arg: FunctionArgExpr,
        operator: FunctionArgOperator,
    },
    Unnamed(FunctionArgExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionArgOperator {
    Equals,
    RightArrow,
    Assignment,
    Colon,
    Value,
    Space,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionArgExpr {
    Expr(Expr),
    QualifiedWildcard(ObjectName),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WindowType {
    WindowSpec(WindowSpec),
    NamedWindow(Ident),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowSpec {
    pub window_name: Option<Ident>,
    pub partition_by: Vec<Expr>,
    pub order_by: Vec<OrderByExpr>,
    pub window_frame: Option<WindowFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowFrame {
    pub units: WindowFrameUnits,
    pub start_bound: WindowFrameBound,
    pub end_bound: Option<WindowFrameBound>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowFrameUnits {
    Rows,
    Range,
    Groups,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WindowFrameBound {
    CurrentRow,
    Preceding(Option<Box<Expr>>),
    Following(Option<Box<Expr>>),
}
