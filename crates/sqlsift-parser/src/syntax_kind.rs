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
    SelectStmt,
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
