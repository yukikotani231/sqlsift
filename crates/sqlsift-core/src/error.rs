//! Error and diagnostic types

use miette::SourceSpan;
use serde::{Deserialize, Serialize};

/// Source location span
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Byte offset of the start of the span in the source. Diagnostics returned
    /// by the analyzer always have it set (it matches `line` / `column`); spans
    /// created with [`Span::with_location`] or [`Span::from_sqlparser`] start
    /// with 0 until the analyzer fills it in.
    pub offset: usize,
    /// Length in bytes
    pub length: usize,
    /// Line number (1-indexed)
    pub line: usize,
    /// Column number (1-indexed)
    pub column: usize,
}

impl Span {
    /// Create a span with byte offset (for backwards compatibility)
    pub fn new(offset: usize, length: usize) -> Self {
        Self {
            offset,
            length,
            line: 0,
            column: 0,
        }
    }

    /// Create a span with line and column information
    pub fn with_location(line: usize, column: usize, length: usize) -> Self {
        Self {
            offset: 0,
            length,
            line,
            column,
        }
    }

    /// Create a span from sqlparser's Span
    pub fn from_sqlparser(span: &sqlparser::tokenizer::Span) -> Self {
        let start = span.start;
        let end = span.end;
        let length = if end.column > start.column {
            end.column as usize - start.column as usize
        } else {
            1
        };
        Self {
            offset: 0,
            length,
            line: start.line as usize,
            column: start.column as usize,
        }
    }
}

impl From<Span> for SourceSpan {
    fn from(span: Span) -> Self {
        SourceSpan::new(span.offset.into(), span.length)
    }
}

/// Diagnostic severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Diagnostic message for SQL analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
    pub help: Option<String>,
    pub labels: Vec<Label>,
    /// Name of the query the diagnostic is in, from a sqlc-style
    /// `-- name: GetPost :one` comment before its statement
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_name: Option<String>,
}

/// Label for source annotations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub message: String,
    pub span: Span,
}

impl Diagnostic {
    pub fn error(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            severity: Severity::Error,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            query_name: None,
        }
    }

    pub fn warning(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            help: None,
            labels: Vec::new(),
            query_name: None,
        }
    }

    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn with_label(mut self, message: impl Into<String>, span: Span) -> Self {
        self.labels.push(Label {
            message: message.into(),
            span,
        });
        self
    }

    /// Get the error code string (e.g., "E0001")
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

/// Types of diagnostics (one per rule; see [`crate::rules::RULES`])
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// E0001: Table not found
    TableNotFound,
    /// E0002: Column not found
    ColumnNotFound,
    /// E0003: Type mismatch
    TypeMismatch,
    /// E0004: Potential NOT NULL violation
    PotentialNullViolation,
    /// E0005: Column count mismatch (INSERT, column aliases, subqueries, rows)
    ColumnCountMismatch,
    /// E0006: Ambiguous column reference
    AmbiguousColumn,
    /// E0007: JOIN type mismatch
    JoinTypeMismatch,
    /// E0008: INSERT omits a NOT NULL column that has no default
    MissingRequiredColumn,
    /// E0009: A table alias or CTE name given twice in one query block
    DuplicateName,
    /// E0010: A column given twice in an INSERT column list or UPDATE SET
    DuplicateTargetColumn,
    /// E0011: ORDER BY / GROUP BY position past the end of the select list
    PositionOutOfRange,
    /// E0012: Aggregate or window function in a clause that doesn't allow it
    MisplacedAggregate,
    /// E0013: A value given for a generated column by INSERT or UPDATE
    GeneratedColumnWrite,
    /// E0014: ON CONFLICT target that matches no primary key or unique constraint
    UnmatchedConflictTarget,
    /// E0015: ORDER BY of SELECT DISTINCT on something not selected
    DistinctOrderBy,
    /// E0016: Column neither grouped nor aggregated in an aggregate query
    GroupingError,
    /// E0017: A literal the target column's type can't hold
    ValueOutOfRange,
    /// E0018: Comparison with NULL using `=`, `<>`, ... (never true)
    NullComparison,
    /// E0019: NOT IN over a subquery whose column may be NULL
    NotInWithNulls,
    /// E0020: Column of an IN subquery's select list that is the outer query's
    OuterColumnInSubquery,
    /// E0021: Comma-separated FROM tables with no condition linking them
    MissingJoinCondition,
    /// E0022: A condition that is always or never true from the schema
    ConstantCondition,
    /// E0023: WHERE condition that turns an outer join into an inner join
    OuterJoinFiltered,
    /// E0024: UPDATE or DELETE without WHERE
    UnfilteredWrite,
    /// E0025: INSERT without a column list
    InsertWithoutColumns,
    /// E0026: SELECT * in a statement's result
    SelectStar,
    /// E0027: A built-in function or operator given an argument type it doesn't take
    WrongArgumentType,
    /// E0028: LIMIT / OFFSET / FETCH FIRST without ORDER BY
    LimitWithoutOrderBy,
    /// E0029: Type mismatch the database converts implicitly (MySQL, SQLite)
    ImplicitConversion,
    /// E0030: JOIN condition that swaps a column of the foreign key between the
    /// tables for another key column
    ForeignKeyJoinMismatch,
    /// E0031: Aggregate over a table whose rows another one-to-many join multiplies
    JoinFanOut,
    /// E0032: Division of integer columns truncates toward zero
    IntegerDivision,
    /// E0033: Comparing or assigning timestamp with timestamptz depends on session TimeZone
    TimestampTimezoneMismatch,
    /// E0034: CASE over an enum column misses some labels and has no ELSE
    CaseEnumMissingLabels,
    /// E0035: Duplicate primary key or unique values in an INSERT VALUES list
    DuplicateInsertValue,
    /// E0037: Literal that violates a CHECK constraint in INSERT VALUES or UPDATE SET
    CheckConstraintViolation,
    /// Parse error
    ParseError,
}

impl DiagnosticKind {
    /// Rule code (e.g. `E0002`)
    pub fn code(&self) -> &'static str {
        self.rule().code
    }

    /// Rule name (e.g. `column-not-found`)
    pub fn name(&self) -> &'static str {
        self.rule().name
    }
}
