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
    /// E0005: Column count mismatch in INSERT
    ColumnCountMismatch,
    /// E0006: Ambiguous column reference
    AmbiguousColumn,
    /// E0007: JOIN type mismatch
    JoinTypeMismatch,
    /// E0008: INSERT omits a NOT NULL column that has no default
    MissingRequiredColumn,
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
