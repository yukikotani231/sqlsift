//! Error and diagnostic types

use miette::SourceSpan;
use serde::{Deserialize, Serialize};

/// Source location span
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Byte offset from start of source (optional, for miette compatibility)
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
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

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

/// Types of diagnostics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// E0001: Table not found
    TableNotFound,
    /// E0002: Column not found
    ColumnNotFound,
    /// E0003: Type mismatch
    TypeMismatch,
    /// E0004: Potential NOT NULL violation
    PotentialNullViolation,
    /// E0005: Column count mismatch in INSERT
    ColumnCountMismatch,
    /// E0006: Ambiguous column reference
    AmbiguousColumn,
    /// E0007: JOIN type mismatch
    JoinTypeMismatch,
    /// Parse error
    ParseError,
}

impl DiagnosticKind {
    pub fn code(&self) -> &'static str {
        match self {
            DiagnosticKind::TableNotFound => "E0001",
            DiagnosticKind::ColumnNotFound => "E0002",
            DiagnosticKind::TypeMismatch => "E0003",
            DiagnosticKind::PotentialNullViolation => "E0004",
            DiagnosticKind::ColumnCountMismatch => "E0005",
            DiagnosticKind::AmbiguousColumn => "E0006",
            DiagnosticKind::JoinTypeMismatch => "E0007",
            DiagnosticKind::ParseError => "E1000",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            DiagnosticKind::TableNotFound => "table-not-found",
            DiagnosticKind::ColumnNotFound => "column-not-found",
            DiagnosticKind::TypeMismatch => "type-mismatch",
            DiagnosticKind::PotentialNullViolation => "potential-null-violation",
            DiagnosticKind::ColumnCountMismatch => "column-count-mismatch",
            DiagnosticKind::AmbiguousColumn => "ambiguous-column",
            DiagnosticKind::JoinTypeMismatch => "join-type-mismatch",
            DiagnosticKind::ParseError => "parse-error",
        }
    }
}
