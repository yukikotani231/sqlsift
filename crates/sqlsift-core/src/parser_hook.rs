//! Experimental (#257): a statement parser that reads PostgreSQL text before sqlparser.
//!
//! The in-house parser (`crates/sqlsift-parser`) is set here by the CLI when it is built
//! with it and asked for it, so the analyzer and the schema builder can run on its output
//! and their diagnostics be compared with sqlparser's. Without a parser set, nothing
//! changes.

use std::ops::Range;
use std::sync::OnceLock;

use sqlparser::ast::Statement;

/// What the parser made of one statement.
#[derive(Debug)]
pub enum Parsed {
    /// The statement, with locations relative to the whole text.
    Statement(Box<Statement>),
    /// Valid, but not given as sqlparser's AST: sqlparser parses this statement's text
    /// as before, and if it can't, the statement is skipped without a parse error.
    Fallback,
    /// A syntax error at byte `offset` of the whole text.
    SyntaxError { message: String, offset: usize },
}

/// One statement: its byte range in the text (without the `;`) and what it parsed to.
#[derive(Debug)]
pub struct ParsedStatement {
    pub range: Range<usize>,
    pub parsed: Parsed,
}

/// Splits a whole text into statements and parses them.
pub type StatementParser = fn(&str) -> Vec<ParsedStatement>;

static PARSER: OnceLock<StatementParser> = OnceLock::new();

/// Sets the parser for PostgreSQL text, for the rest of the process. Returns `false` if
/// one was set before.
pub fn set_statement_parser(parser: StatementParser) -> bool {
    PARSER.set(parser).is_ok()
}

pub(crate) fn statement_parser() -> Option<StatementParser> {
    PARSER.get().copied()
}

/// The 1-based line and column (in characters) of byte `offset` of `text`.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}
