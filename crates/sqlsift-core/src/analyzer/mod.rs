//! SQL analyzer module

mod comment_directives;
mod resolver;
mod scope;
mod type_check;

use std::ops::Range;

use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::Dialect;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::mysql;
use crate::psql::{self, Preprocessed};
use crate::rules::RuleConfig;
use crate::schema::{mask_unsupported_clauses, unparsed_definition};
use crate::schema::{Catalog, QualifiedName, SchemaBuilder, SkippedDefinition};
use crate::sqlc::{self, QueryNames};
use crate::templating::{self, Template, Templating};

use comment_directives::InlineDirectives;
use resolver::Resolver;

/// SQL Analyzer - validates SQL against a schema catalog
pub struct Analyzer<'a> {
    catalog: &'a Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
    rules: RuleConfig,
    templating: Templating,
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
            rules: RuleConfig::default(),
            templating: Templating::None,
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
            rules: RuleConfig::default(),
            templating: Templating::None,
        }
    }

    /// Report rules at the configured levels (rules that are off are not reported)
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::rules::{RuleConfig, RuleLevel};
    /// use sqlsift_core::schema::Catalog;
    ///
    /// let catalog = Catalog::default();
    /// let mut rules = RuleConfig::default();
    /// rules.configure("table-not-found", RuleLevel::Off).unwrap();
    /// let mut analyzer = Analyzer::new(&catalog).with_rules(rules);
    /// assert!(analyzer.analyze("SELECT 1 FROM missing").is_empty());
    /// ```
    #[must_use]
    pub fn with_rules(mut self, rules: RuleConfig) -> Self {
        self.rules = rules;
        self
    }

    /// Treat query text as a template (e.g. dbt models with [`Templating::Jinja`]):
    /// template syntax is masked before parsing, keeping every location
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::Catalog;
    /// use sqlsift_core::Templating;
    ///
    /// let catalog = Catalog::default();
    /// let mut analyzer = Analyzer::new(&catalog).with_templating(Templating::Jinja);
    /// assert!(analyzer.analyze("SELECT id FROM {{ ref('orders') }}").is_empty());
    /// ```
    #[must_use]
    pub fn with_templating(mut self, templating: Templating) -> Self {
        self.templating = templating;
        self
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
        let (sql, bom) = strip_bom(sql);
        let mut diagnostics = self.analyze_text(sql, sql, &[]);
        shift_offsets(&mut diagnostics, bom);
        diagnostics
    }

    /// Analyze the SQL in a TypeScript or JavaScript file: the tagged template
    /// literals whose tag is one of `tags` (see [`crate::embedded`]). Diagnostics
    /// point at the query's location in `source`.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let source = "const user = await sql`SELECT nme FROM users WHERE id = ${id}`;";
    /// let diagnostics = Analyzer::new(&catalog).analyze_embedded(source, &["sql"]);
    /// assert_eq!(diagnostics.len(), 1);
    /// assert_eq!(diagnostics[0].span.unwrap().column, 31);
    /// ```
    pub fn analyze_embedded<S: AsRef<str>>(&mut self, source: &str, tags: &[S]) -> Vec<Diagnostic> {
        let (source, bom) = strip_bom(source);
        let extracted =
            crate::embedded::extract(source, tags, self.dialect, crate::embedded::Host::Script);
        let mut diagnostics = self.analyze_text(&extracted.text, source, &extracted.identifiers);
        shift_offsets(&mut diagnostics, bom);
        diagnostics
    }

    /// Analyze the SQL in the `<script>` blocks of a Vue or Svelte component, as
    /// [`Analyzer::analyze_embedded`] does for a TypeScript or JavaScript file
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let source = "<script setup>\nconst u = await sql`SELECT nme FROM users`;\n</script>\n<p>`sql`</p>";
    /// let diagnostics = Analyzer::new(&catalog).analyze_embedded_component(source, &["sql"]);
    /// assert_eq!(diagnostics.len(), 1);
    /// ```
    pub fn analyze_embedded_component<S: AsRef<str>>(
        &mut self,
        source: &str,
        tags: &[S],
    ) -> Vec<Diagnostic> {
        let (source, bom) = strip_bom(source);
        let extracted =
            crate::embedded::extract(source, tags, self.dialect, crate::embedded::Host::Component);
        let mut diagnostics = self.analyze_text(&extracted.text, source, &extracted.identifiers);
        shift_offsets(&mut diagnostics, bom);
        diagnostics
    }

    /// Analyze `sql`, which has the same lines and character columns as `original`
    /// (where byte offsets point). Name diagnostics on the `substituted` identifiers
    /// ((line, first column, end column)) are dropped.
    fn analyze_text(
        &mut self,
        sql: &str,
        original: &str,
        substituted: &[(usize, usize, usize)],
    ) -> Vec<Diagnostic> {
        self.diagnostics.clear();

        // Template tags, then psql meta-commands and variables (the rewrites keep
        // every line and character column)
        let catalog = self.catalog;
        let template = match self.templating {
            Templating::Jinja => templating::mask_jinja(sql, &|schema, table| {
                let name = QualifiedName::with_schema(schema, table);
                catalog.table_exists(&name) || catalog.view_exists(&name)
            }),
            Templating::None => Template::unchanged(sql),
        };
        let source = match self.dialect {
            SqlDialect::PostgreSQL => psql::preprocess(&template.masked.text),
            SqlDialect::MySQL => mysql::preprocess(&template.masked.text),
            SqlDialect::SQLite => Preprocessed::unchanged(&template.masked.text),
        };
        // sqlc parameters (`sqlc.arg(name)`, `@name`) become placeholders
        let text = sqlc::mask_parameters(&source.text, self.dialect);
        // A file that looks like a template but isn't masked doesn't parse
        let template_hint =
            self.templating == Templating::None && templating::looks_like_jinja(sql);

        // Parse inline disable directives from comments (`{# ... #}` comments too)
        let directives = InlineDirectives::parse(&template.directive_text());
        // sqlc query names (`-- name: GetPost :one`)
        let query_names = QueryNames::parse(sql);

        // Parse the SQL
        let lines = LineIndex::new(&text);
        let (statements, mut skipped) = self.parse_statements(&text, &lines);
        // Applied in source order, before the statements that follow them
        skipped.reverse();

        // Tables, views and types created, altered or dropped by the file's own
        // statements, applied to a copy of the catalog made on the first such
        // statement, so they are visible to the later statements of this file only
        let mut file_schema: Option<SchemaBuilder> = None;

        // Analyze each statement
        for (stmt, origin) in &statements {
            // Definitions before this statement that could not be parsed
            while skipped
                .last()
                .is_some_and(|(at, _)| (at.line, at.column) < (origin.line, origin.column))
            {
                if let Some((_, definition)) = skipped.pop() {
                    file_schema
                        .get_or_insert_with(|| {
                            SchemaBuilder::from_catalog(self.catalog.clone(), self.dialect)
                        })
                        .skip_definition(definition);
                }
            }

            let catalog = file_schema
                .as_ref()
                .map_or(self.catalog, SchemaBuilder::catalog);

            // Name resolution and type checking in one walk over the statement
            let mut resolver = Resolver::new(catalog, self.dialect);
            resolver.statement(stmt);

            // Locations relative to the input
            for mut diagnostic in resolver.into_diagnostics() {
                if let Some(span) = diagnostic.span.as_mut() {
                    origin.shift(span);
                }
                self.diagnostics.push(diagnostic);
            }

            let to_record: Vec<(QualifiedName, usize)> = if let Statement::Drop {
                object_type: ObjectType::Table | ObjectType::View,
                names,
                ..
            } = stmt
            {
                names
                    .iter()
                    .filter_map(|name| {
                        let qualified = catalog.qualified_name(name);
                        if catalog.table_exists(&qualified) || catalog.view_exists(&qualified) {
                            let drop_line = if let Some(id) = name.0.first() {
                                let mut span = Span::from_sqlparser(&id.span);
                                origin.shift(&mut span);
                                if span.line > 0 {
                                    span.line
                                } else {
                                    origin.line
                                }
                            } else {
                                origin.line
                            };
                            Some((qualified, drop_line))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };

            if !to_record.is_empty() {
                let schema = file_schema.get_or_insert_with(|| {
                    SchemaBuilder::from_catalog(self.catalog.clone(), self.dialect)
                });
                for (qualified, drop_line) in to_record {
                    schema.record_dropped_relation(qualified, drop_line);
                }
            }

            if SchemaBuilder::changes_schema(stmt) {
                file_schema
                    .get_or_insert_with(|| {
                        SchemaBuilder::from_catalog(self.catalog.clone(), self.dialect)
                    })
                    .apply_statement(stmt);
            }
        }

        // Report diagnostics in source order (parse errors are found before analysis)
        self.diagnostics
            .sort_by_key(|d| d.span.map_or((usize::MAX, 0), |s| (s.line, s.column)));

        // Spans are built from line/column locations: add their byte offsets in the
        // original input (extracting embedded SQL or masking a template may change
        // byte lengths, never lines or character columns)
        let original_lines = (!std::ptr::eq(original, sql)
            || template.masked.text.len() != sql.len()
            || source.text.len() != sql.len())
        .then(|| LineIndex::new(original));
        let offsets = original_lines.as_ref().unwrap_or(&lines);
        for diagnostic in &mut self.diagnostics {
            let labels = diagnostic.labels.iter_mut().map(|l| &mut l.span);
            for span in diagnostic.span.iter_mut().chain(labels) {
                offsets.fill_offset(span);
            }
        }

        // Filter out diagnostics suppressed by inline or file directives, then apply rule levels.
        // A diagnostic that a directive meant to suppress but misspelled the rule of
        // says so.
        let diagnostics = std::mem::take(&mut self.diagnostics)
            .into_iter()
            .filter_map(|mut d| {
                let Some(span) = d.span else {
                    return (!directives.is_suppressed_in_file(d.kind)).then_some(d);
                };
                // A table or column name interpolated by psql (`FROM :"tbl"`), a
                // template (`FROM {{ ref('tbl') }}`) or a template literal
                // (`FROM ${tbl}`) is unknown
                let substituted = matches!(
                    d.kind,
                    DiagnosticKind::TableNotFound
                        | DiagnosticKind::ColumnNotFound
                        | DiagnosticKind::AmbiguousColumn
                ) && (source.is_substituted(span.line, span.column)
                    || template.masked.is_substituted(span.line, span.column)
                    || psql::is_within(substituted, span.line, span.column));
                if substituted || directives.is_suppressed(d.kind, span.line) {
                    return None;
                }
                d.query_name = query_names.at(span.line).map(str::to_string);
                if d.kind == DiagnosticKind::ParseError {
                    explain_template_parse_error(&mut d, &template, template_hint);
                }
                if let Some(note) = directives.unknown_id_help(span.line) {
                    d.help = Some(match d.help.take() {
                        Some(help) => format!("{help}\n{note}"),
                        None => note,
                    });
                }
                Some(d)
            })
            .collect();
        self.rules.apply(diagnostics)
    }

    /// Parse `sql` into statements, each with the position where its text starts.
    ///
    /// If the input doesn't parse as a whole, each statement is parsed on its own, so
    /// a syntax error is reported where it occurs and doesn't hide diagnostics in the
    /// other statements. Only the statement's own text is parsed (keeping this linear
    /// in the input size); its locations are shifted by its [`Origin`] afterwards.
    /// A CREATE TABLE / VIEW that doesn't parse is retried without clauses that don't
    /// change its columns (`UNLOGGED`, `WITH NO DATA`, ...); if it still doesn't
    /// parse, the table or view it defines is returned with its position.
    fn parse_statements(&mut self, sql: &str, lines: &LineIndex) -> ParsedStatements {
        let dialect = self.dialect.parser_dialect();
        let error = match Parser::parse_sql(dialect.as_ref(), sql) {
            Ok(statements) => {
                let statements = statements.into_iter().map(|s| (s, Origin::START)).collect();
                return (statements, Vec::new());
            }
            Err(error) => error,
        };

        let Some(ranges) = statement_ranges(dialect.as_ref(), sql, lines) else {
            // Tokenizer error: nothing can be parsed reliably
            self.diagnostics
                .push(parse_error_diagnostic(&error, sql, 0..sql.len(), lines));
            return (Vec::new(), Vec::new());
        };

        let mut statements = Vec::new();
        let mut skipped = Vec::new();
        for range in ranges {
            let (line, column) = lines.line_column(range.start);
            let origin = Origin { line, column };
            let text = &sql[range.clone()];
            let parsed = Parser::parse_sql(dialect.as_ref(), text).or_else(|error| {
                // MySQL's INSERT ... SET, which sqlparser doesn't support
                match self.dialect {
                    SqlDialect::MySQL => mysql::parse_insert_set(dialect.as_ref(), text)
                        .map_or(Err(error), |parsed| parsed.map(|s| vec![s])),
                    SqlDialect::PostgreSQL | SqlDialect::SQLite => Err(error),
                }
            });
            let error = match parsed {
                Ok(parsed) => {
                    statements.extend(parsed.into_iter().map(|s| (s, origin)));
                    continue;
                }
                Err(error) => error,
            };
            let retried = mask_unsupported_clauses(self.dialect, text, false)
                .and_then(|(masked, _)| Parser::parse_sql(dialect.as_ref(), &masked).ok());
            if let Some(parsed) = retried {
                statements.extend(parsed.into_iter().map(|s| (s, origin)));
                continue;
            }
            if let Some((kind, name)) = unparsed_definition(self.dialect, text) {
                let start = range.start + (text.len() - text.trim_start().len());
                skipped.push((
                    origin,
                    SkippedDefinition {
                        kind,
                        name: Some(name),
                        line: Some(lines.line_column(start).0),
                    },
                ));
            }
            self.diagnostics
                .push(parse_error_diagnostic(&error, sql, range, lines));
        }
        (statements, skipped)
    }
}

/// Make a parse error in a template name the template tag it is on instead of its
/// masked text (`found: $1`), and suggest what to do. `not_masked`: the file looks
/// like a template but templating is off.
fn explain_template_parse_error(d: &mut Diagnostic, template: &Template, not_masked: bool) {
    let help = if not_masked {
        "this looks like a Jinja (dbt) template: check it with `--templating jinja` \
         (or `templating = \"jinja\"` in sqlsift.toml)"
            .to_string()
    } else {
        let Some(tag) = d.span.and_then(|s| template.tag_at(s.line, s.column)) else {
            return;
        };
        if let Some(found) = d.message.rfind("found: ") {
            d.message.truncate(found);
            d.message.push_str("found: Jinja expression ");
            d.message.push_str(tag);
        }
        "sqlsift can't tell what SQL this template expression expands to here; \
         skip the file with `ignore` in sqlsift.toml or `{# sqlsift:disable-file #}`"
            .to_string()
    };
    d.help = Some(match d.help.take() {
        Some(existing) => format!("{existing}\n{help}"),
        None => help,
    });
}

/// Parsed statements and the table / view definitions that could not be parsed,
/// each with the position where its text starts
type ParsedStatements = (Vec<(Statement, Origin)>, Vec<(Origin, SkippedDefinition)>);

/// `text` without a leading UTF-8 byte order mark (which editors on Windows and
/// tools like SSMS write), and the byte length removed. Columns of the stripped
/// text are the ones an editor shows; see [`shift_offsets`] for byte offsets.
pub(crate) fn strip_bom(text: &str) -> (&str, usize) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => (rest, text.len() - rest.len()),
        None => (text, 0),
    }
}

/// Make the byte offsets of diagnostics found in a text [`strip_bom`] removed
/// `bom` bytes from relative to the original text
pub(crate) fn shift_offsets(diagnostics: &mut [Diagnostic], bom: usize) {
    if bom == 0 {
        return;
    }
    for diagnostic in diagnostics {
        let labels = diagnostic.labels.iter_mut().map(|l| &mut l.span);
        for span in diagnostic.span.iter_mut().chain(labels) {
            if span.line > 0 {
                span.offset += bom;
            }
        }
    }
}

/// Where a statement's text starts in the full input (1-indexed line and column)
#[derive(Debug, Clone, Copy)]
struct Origin {
    line: usize,
    column: usize,
}

impl Origin {
    const START: Origin = Origin { line: 1, column: 1 };

    /// Convert a span relative to the statement's text into one relative to the input
    fn shift(&self, span: &mut Span) {
        if span.line == 0 {
            return;
        }
        if span.line == 1 {
            span.column += self.column - 1;
        }
        span.line += self.line - 1;
    }
}

/// Byte offsets of line starts, for converting between offsets and line/column
struct LineIndex<'a> {
    sql: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(sql: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(sql.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { sql, starts }
    }

    /// Byte offset of a 1-indexed line / character column
    fn byte_offset(&self, line: u64, column: u64) -> usize {
        let Some(&line_start) = self.starts.get(line.saturating_sub(1) as usize) else {
            return self.sql.len();
        };
        self.sql[line_start..]
            .char_indices()
            .nth(column.saturating_sub(1) as usize)
            .map_or(self.sql.len(), |(i, _)| line_start + i)
    }

    /// Set the byte offset of a span that has a line/column location
    fn fill_offset(&self, span: &mut Span) {
        if span.line > 0 {
            span.offset = self.byte_offset(span.line as u64, span.column as u64);
        }
    }

    /// 1-indexed line / character column of a byte offset
    fn line_column(&self, offset: usize) -> (usize, usize) {
        let line = self.starts.partition_point(|&start| start <= offset);
        let line_start = self.starts[line - 1];
        (line, self.sql[line_start..offset].chars().count() + 1)
    }
}

/// Byte ranges of the `;`-separated statements in `sql`, or `None` if it can't be tokenized
fn statement_ranges(
    dialect: &dyn Dialect,
    sql: &str,
    lines: &LineIndex,
) -> Option<Vec<Range<usize>>> {
    let tokens = Tokenizer::new(dialect, sql).tokenize_with_location().ok()?;
    let mut ranges = Vec::new();
    let mut start = 0;
    for token in tokens {
        if token.token == Token::SemiColon {
            let end = lines.byte_offset(token.span.start.line, token.span.start.column) + 1;
            ranges.push(start..end);
            start = end;
        }
    }
    if start < sql.len() {
        ranges.push(start..sql.len());
    }
    Some(ranges)
}

/// Build an E1000 diagnostic from a parser error within the statement at `range`
fn parse_error_diagnostic(
    error: &ParserError,
    sql: &str,
    range: Range<usize>,
    lines: &LineIndex,
) -> Diagnostic {
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
    let span = if let Some((line, column)) = location.filter(|(l, _)| *l > 0) {
        let (start_line, start_column) = lines.line_column(range.start);
        let mut span = Span::with_location(line, column, 1);
        Origin {
            line: start_line,
            column: start_column,
        }
        .shift(&mut span);
        span
    } else {
        let text = &sql[range.clone()];
        let end = range.start + text.trim_end().trim_end_matches(';').trim_end().len();
        let (line, column) = lines.line_column(end);
        Span::with_location(line, column, 1)
    };
    Diagnostic::error(
        DiagnosticKind::ParseError,
        format!("Parse error: {message}"),
    )
    .with_span(span)
}
/// Output columns (name and type) of a query, as far as they can be inferred, or
/// `None` if they can't be determined (e.g. `SELECT *` over an unknown table).
/// Used to infer the columns of views and `CREATE TABLE ... AS`.
pub(crate) fn query_output_columns(
    catalog: &Catalog,
    dialect: SqlDialect,
    query: &sqlparser::ast::Query,
) -> Option<Vec<(String, crate::types::SqlType)>> {
    let mut resolver = Resolver::new(catalog, dialect);
    let columns = resolver.query(query, false)?;
    Some(
        columns
            .into_iter()
            .map(|c| (c.name, c.ty.to_sql_type()))
            .collect(),
    )
}
