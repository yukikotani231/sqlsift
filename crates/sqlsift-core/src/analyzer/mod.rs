//! SQL analyzer module

mod comment_directives;
mod resolver;
mod type_resolver;

use std::ops::Range;

use sqlparser::ast::Statement;
use sqlparser::dialect::Dialect;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::schema::Catalog;

use comment_directives::InlineDirectives;
pub use resolver::NameResolver;
use type_resolver::TypeResolver;

/// SQL Analyzer - validates SQL against a schema catalog
pub struct Analyzer<'a> {
    catalog: &'a Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
}

impl<'a> Analyzer<'a> {
    /// Create a new analyzer with default PostgreSQL dialect
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::Catalog;
    ///
    /// let catalog = Catalog::default();
    /// let mut analyzer = Analyzer::new(&catalog);
    /// ```
    pub fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect: SqlDialect::default(),
        }
    }

    /// Create a new analyzer with specified SQL dialect
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::dialect::SqlDialect;
    /// use sqlsift_core::schema::Catalog;
    ///
    /// let catalog = Catalog::default();
    /// let mut analyzer = Analyzer::with_dialect(&catalog, SqlDialect::MySQL);
    /// ```
    pub fn with_dialect(catalog: &'a Catalog, dialect: SqlDialect) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect,
        }
    }

    /// Analyze a SQL query and return diagnostics
    ///
    /// Validates SQL against the schema catalog and returns a list of diagnostics.
    /// Returns an empty vector if no issues are found.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::{Catalog, SchemaBuilder};
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let mut analyzer = Analyzer::new(&catalog);
    /// let diagnostics = analyzer.analyze("SELECT id, name FROM users");
    /// assert!(diagnostics.is_empty());
    /// ```
    pub fn analyze(&mut self, sql: &str) -> Vec<Diagnostic> {
        self.diagnostics.clear();

        // Parse inline disable directives from comments
        let directives = InlineDirectives::parse(sql);

        // Parse the SQL
        let statements = self.parse_statements(sql);

        // Analyze each statement
        for stmt in &statements {
            // Phase 1: Name resolution
            let mut resolver = NameResolver::new(self.catalog).with_dialect(self.dialect);
            resolver.resolve_statement(stmt);

            // Phase 2: Type inference and checking
            let mut type_resolver = TypeResolver::new(self.catalog).with_dialect(self.dialect);
            type_resolver.inherit_scope(&resolver);
            type_resolver.check_statement(stmt);

            // Collect diagnostics from both phases
            self.diagnostics.extend(resolver.into_diagnostics());
            self.diagnostics.extend(type_resolver.into_diagnostics());
        }

        // Report diagnostics in source order (parse errors are found before analysis)
        self.diagnostics
            .sort_by_key(|d| d.span.map_or((usize::MAX, 0), |s| (s.line, s.column)));

        // Filter out diagnostics suppressed by inline directives
        std::mem::take(&mut self.diagnostics)
            .into_iter()
            .filter(|d| {
                if let Some(span) = &d.span {
                    !directives.is_suppressed(d.code(), span.line)
                } else {
                    true
                }
            })
            .collect()
    }

    /// Parse `sql` into statements. If the input doesn't parse as a whole, each
    /// statement is parsed on its own, so a syntax error is reported where it occurs
    /// and doesn't hide diagnostics in the other statements.
    fn parse_statements(&mut self, sql: &str) -> Vec<Statement> {
        let dialect = self.dialect.parser_dialect();
        let error = match Parser::parse_sql(dialect.as_ref(), sql) {
            Ok(statements) => return statements,
            Err(error) => error,
        };

        let Some(ranges) = statement_ranges(dialect.as_ref(), sql) else {
            // Tokenizer error: nothing can be parsed reliably
            self.diagnostics
                .push(parse_error_diagnostic(&error, sql, 0..sql.len()));
            return Vec::new();
        };

        let mut statements = Vec::new();
        for range in ranges {
            // Blank out the other statements so that locations stay absolute
            let masked: String = sql
                .char_indices()
                .map(|(i, c)| {
                    if range.contains(&i) || c == '\n' {
                        c
                    } else {
                        ' '
                    }
                })
                .collect();
            match Parser::parse_sql(dialect.as_ref(), &masked) {
                Ok(parsed) => statements.extend(parsed),
                Err(error) => self
                    .diagnostics
                    .push(parse_error_diagnostic(&error, sql, range)),
            }
        }
        statements
    }
}

/// Byte ranges of the `;`-separated statements in `sql`, or `None` if it can't be tokenized
fn statement_ranges(dialect: &dyn Dialect, sql: &str) -> Option<Vec<Range<usize>>> {
    let tokens = Tokenizer::new(dialect, sql).tokenize_with_location().ok()?;
    let mut ranges = Vec::new();
    let mut start = 0;
    for token in tokens {
        if token.token == Token::SemiColon {
            let end = byte_offset(sql, token.span.start.line, token.span.start.column) + 1;
            ranges.push(start..end);
            start = end;
        }
    }
    if start < sql.len() {
        ranges.push(start..sql.len());
    }
    Some(ranges)
}

/// Byte offset of a 1-indexed line / character column
fn byte_offset(sql: &str, line: u64, column: u64) -> usize {
    let line_start: usize = sql
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    sql[line_start..]
        .char_indices()
        .nth(column.saturating_sub(1) as usize)
        .map_or(sql.len(), |(i, _)| line_start + i)
}

/// 1-indexed line / character column of a byte offset
fn line_column(sql: &str, offset: usize) -> (usize, usize) {
    let before = &sql[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// Build an E1000 diagnostic from a parser error within the statement at `range`
fn parse_error_diagnostic(error: &ParserError, sql: &str, range: Range<usize>) -> Diagnostic {
    let message = match error {
        ParserError::TokenizerError(m) | ParserError::ParserError(m) => m.clone(),
        ParserError::RecursionLimitExceeded => "recursion limit exceeded".to_string(),
    };
    // sqlparser appends " at Line: L, Column: C" to the message
    let (message, location) = match message.rsplit_once(" at Line: ") {
        Some((text, location)) => {
            let mut parts = location.split(", Column: ");
            let line = parts.next().and_then(|l| l.trim().parse::<usize>().ok());
            let column = parts.next().and_then(|c| c.trim().parse::<usize>().ok());
            (text.to_string(), line.zip(column))
        }
        None => (message, None),
    };
    // Without a usable location (e.g. unexpected end of input), point at the end of
    // the statement's last token
    let (line, column) = location.filter(|(l, _)| *l > 0).unwrap_or_else(|| {
        let text = &sql[range.clone()];
        let end = range.start + text.trim_end().trim_end_matches(';').trim_end().len();
        line_column(sql, end)
    });
    Diagnostic::error(
        DiagnosticKind::ParseError,
        format!("Parse error: {}", message),
    )
    .with_span(Span::with_location(line, column, 1))
}
