//! Kinds of tokens and nodes in the syntax tree.

/// The kind of a token or a node. Tokens come first; [`SyntaxKind::is_trivia`] marks the
/// ones the parser skips (and the tree keeps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum SyntaxKind {
    // Trivia
    Whitespace,
    LineComment,
    BlockComment,

    // Tokens
    /// Unquoted identifier or keyword. Keywords are not separate kinds: PostgreSQL decides
    /// by position whether a word is a keyword, so the parser checks the text.
    Ident,
    /// `"quoted identifier"` or `U&"..."`
    QuotedIdent,
    /// `'string'`
    String,
    /// `E'string'` (backslash escapes)
    EscapeString,
    /// `N'string'`
    NationalString,
    /// `B'0101'`
    BitString,
    /// `X'1f'`
    HexString,
    /// `U&'string'`
    UnicodeString,
    /// `$tag$ ... $tag$`
    DollarString,
    /// Integer or decimal literal
    Number,
    /// `$1`
    Param,
    /// An operator: `+`, `<=`, `||`, `->>`, `@>`, ...
    Operator,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Dot,
    Colon,
    /// `::`
    DoubleColon,
    /// A character no token starts with
    Unknown,

    // Nodes
    SourceFile,
    /// A statement the parser does not structure: its tokens are kept, nothing is checked.
    OpaqueStmt,
    /// Tokens the parser could not place
    Error,
    /// A query: `[WITH ...] body [ORDER BY] [LIMIT] [OFFSET]`, the body being a
    /// `SimpleSelect`, a `SetOperation` or a `ParenSelect`.
    SelectStmt,
    /// `SELECT ... [FROM] [WHERE] [GROUP BY] [HAVING]`
    SimpleSelect,
    /// `a UNION [ALL] b`, `INTERSECT`, `EXCEPT`
    SetOperation,
    /// A parenthesized query as an operand of a set operation
    ParenSelect,
    /// `INSERT INTO t [AS a] [(cols)] {VALUES ... | query | DEFAULT VALUES} [ON CONFLICT]
    /// [RETURNING]`
    InsertStmt,
    /// `UPDATE t [AS a] SET ... [FROM] [WHERE] [RETURNING]`
    UpdateStmt,
    /// `DELETE FROM t [AS a] [USING] [WHERE] [RETURNING]`
    DeleteStmt,
    /// `OVERRIDING SYSTEM VALUE` / `OVERRIDING USER VALUE`
    OverridingClause,
    /// `ON CONFLICT [target] DO NOTHING | DO UPDATE SET ... [WHERE]`
    OnConflictClause,
    /// `(cols) [WHERE]` or `ON CONSTRAINT name`
    ConflictTarget,
    /// `DO NOTHING` or `DO UPDATE SET ... [WHERE]`
    ConflictAction,
    /// `SET a = 1, (b, c) = (2, 3)`
    SetClause,
    Assignment,
    /// `USING` in DELETE
    UsingClause,
    ReturningClause,
    /// `VALUES (...), (...)`
    ValuesClause,
    ValuesRow,
    WithClause,
    /// `name [(cols)] AS [[NOT] MATERIALIZED] (query)`
    Cte,
    SelectClause,
    TargetList,
    TargetItem,
    Alias,
    FromClause,
    /// A table name with an optional alias
    TableRef,
    /// A parenthesized query in FROM, with an alias
    SubqueryRef,
    /// A parenthesized FROM item (`(a JOIN b ON ...)`)
    ParenFromItem,
    JoinExpr,
    JoinCondition,
    WhereClause,
    GroupByClause,
    HavingClause,
    OrderByClause,
    SortItem,
    LimitClause,
    OffsetClause,
    /// A dotted name (`schema.table`, `t.col`, `t.*`)
    Name,
    ColumnList,
    Literal,
    ParamRef,
    ColumnRef,
    Star,
    BinaryExpr,
    PrefixExpr,
    /// `x IS [NOT] NULL / TRUE / FALSE / UNKNOWN / DISTINCT FROM y`
    IsExpr,
    BetweenExpr,
    InExpr,
    LikeExpr,
    ParenExpr,
    /// A row constructor in parentheses: `(a, b)`
    RowExpr,
    /// `(SELECT ...)` used as a value
    SubqueryExpr,
    ExistsExpr,
    CastExpr,
    /// `x::type`
    TypeCastExpr,
    TypeName,
    CaseExpr,
    WhenClause,
    ElseClause,
    FuncCall,
    ArgList,
    /// `ANY (...)`, `ALL (...)`, `SOME (...)` on the right of a comparison
    QuantifiedExpr,
    /// `WITHIN GROUP (ORDER BY ...)`
    WithinGroupClause,
    /// `FILTER (WHERE ...)`
    FilterClause,
    /// `OVER name` or `OVER (...)`
    OverClause,
    /// `([name] [PARTITION BY ...] [ORDER BY ...] [frame])`
    WindowSpec,
    PartitionByClause,
    /// `ROWS / RANGE / GROUPS [BETWEEN] bound [AND bound] [EXCLUDE ...]`
    FrameClause,
    FrameBound,
    /// `WINDOW name AS (...), ...`
    WindowClause,
    WindowDef,
    /// A function in FROM: `generate_series(1, 3) [WITH ORDINALITY] [alias]`
    FuncTableRef,
    /// `ARRAY[...]`, or `[...]` nested in one
    ArrayExpr,
    /// `ARRAY(SELECT ...)`
    ArraySubquery,
    /// `DATE '2024-01-01'`, `INTERVAL '1 day'`: a type name and a string
    TypedLiteral,
    /// `EXTRACT(field FROM x)`
    ExtractExpr,
    /// `x COLLATE name`
    CollateExpr,
    /// `x AT TIME ZONE y`
    AtTimeZoneExpr,
    /// `(x).field`, `(x).*`
    FieldAccessExpr,
    /// `DEFAULT` as a value in `VALUES` or `SET`
    DefaultExpr,
    /// `x[i]`, `x[a:b]`
    SubscriptExpr,
    ExprList,
}

impl SyntaxKind {
    /// Whitespace and comments.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::Whitespace | SyntaxKind::LineComment | SyntaxKind::BlockComment
        )
    }

    /// Whether the kind is a token (as opposed to a node).
    pub fn is_token(self) -> bool {
        self <= SyntaxKind::Unknown
    }
}
