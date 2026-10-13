//! SQL analyzer module

pub(crate) mod bigquery;
mod comment_directives;
mod describe;
mod fan_out;
mod grouping;
mod nullability;
mod parameters;
mod ranges;
mod resolver;
mod restriction;
mod scope;
mod signatures;
mod snowflake;
mod structure;
mod suspicious;
mod syntax;
mod type_check;

use std::ops::Range;

use sqlparser::dialect::Dialect;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, Tokenizer};
use sqlsift_ast::{Spanned, Statement};

use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::mysql;
use crate::named_queries;
use crate::psql::{self, Preprocessed};
use crate::rules::RuleConfig;
use crate::schema::{mask_unsupported_clauses, unparsed_definition};
use crate::schema::{Catalog, QualifiedName, SchemaBuilder, SkippedDefinition};
use crate::sqlc::{self, QueryNames};
use crate::templating::{self, Template, Templating};

use comment_directives::InlineDirectives;
pub use describe::{Description, OutputColumn, Parameter, QueryDescription, QueryKind};
use resolver::Resolver;

/// A use of a catalog table or view, or of its columns, by a query: what
/// [`Analyzer::with_references`] records. Only names that resolve to the
/// catalog are recorded (not CTEs, subqueries or missing tables); a column is
/// recorded where it is resolved, so a CTE or subquery over a table records the
/// table's columns it reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Reference {
    /// The table or view, by its schema-qualified name
    pub relation: QualifiedName,
    pub kind: ReferenceKind,
    /// Where it is used: the table name, the column reference or the `*`
    pub span: Span,
    /// Whether a column is written (a target of INSERT or UPDATE SET; every
    /// column of an INSERT without a column list) rather than read
    pub write: bool,
}

/// What a [`Reference`] uses
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "column", rename_all = "snake_case")]
pub enum ReferenceKind {
    /// The relation itself (FROM, JOIN, the target of INSERT / UPDATE / DELETE)
    Relation,
    /// One of its columns
    Column(String),
    /// Every column (`SELECT *`, `t.*`)
    AllColumns,
}

/// SQL Analyzer - validates SQL against a schema catalog
pub struct Analyzer<'a> {
    catalog: &'a Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
    rules: RuleConfig,
    templating: Templating,
    /// Statements described while analyzing (only collected by `describe*`)
    descriptions: Option<Vec<QueryDescription>>,
    /// References recorded by the last analysis, if they are recorded
    references: Option<Vec<Reference>>,
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
            descriptions: None,
            references: None,
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
            descriptions: None,
            references: None,
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

    /// Record the [`Reference`]s of the queries to catalog tables, views and
    /// columns, returned by [`Analyzer::references`] after an analysis
    ///
    /// ```
    /// use sqlsift_core::analyzer::{Analyzer, ReferenceKind};
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let mut analyzer = Analyzer::new(&catalog).with_references();
    /// analyzer.analyze("SELECT u.name FROM users u");
    /// let kinds: Vec<&ReferenceKind> = analyzer.references().iter().map(|r| &r.kind).collect();
    /// assert_eq!(kinds, [&ReferenceKind::Column("name".into()), &ReferenceKind::Relation]);
    /// ```
    #[must_use]
    pub fn with_references(mut self) -> Self {
        self.references = Some(Vec::new());
        self
    }

    /// The references recorded by the last analysis, in source order (empty
    /// unless [`Analyzer::with_references`] turned recording on)
    pub fn references(&self) -> &[Reference] {
        self.references.as_deref().unwrap_or_default()
    }

    fn shift_references(&mut self, bom: usize) {
        if bom == 0 {
            return;
        }
        for reference in self.references.iter_mut().flatten() {
            if reference.span.line > 0 {
                reference.span.offset += bom;
            }
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
        let (sql, bom) = strip_bom(sql);
        let mut diagnostics = self.analyze_text(sql, sql, &[], &[]);
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
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
        let mut diagnostics = self.analyze_text(
            &extracted.text,
            source,
            &extracted.identifiers,
            &extracted.fragments,
        );
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
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
        let mut diagnostics = self.analyze_text(
            &extracted.text,
            source,
            &extracted.identifiers,
            &extracted.fragments,
        );
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
        diagnostics
    }

    /// Analyze the SQL in a Rust file: the string literals passed to sqlx's query
    /// macros and functions named in `macros` (see [`crate::sqlx`]). Diagnostics
    /// point at the query's location in `source`.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::SchemaBuilder;
    /// use sqlsift_core::sqlx::DEFAULT_MACROS;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let source = r#"let user = sqlx::query!("SELECT nme FROM users WHERE id = $1", id);"#;
    /// let diagnostics = Analyzer::new(&catalog).analyze_rust(source, DEFAULT_MACROS);
    /// assert_eq!(diagnostics.len(), 1);
    /// assert_eq!(diagnostics[0].span.unwrap().column, 33);
    /// ```
    pub fn analyze_rust<S: AsRef<str>>(&mut self, source: &str, macros: &[S]) -> Vec<Diagnostic> {
        let (source, bom) = strip_bom(source);
        let extracted = crate::sqlx::extract(source, macros);
        let mut diagnostics = self.analyze_text(
            &extracted.text,
            source,
            &extracted.identifiers,
            &extracted.fragments,
        );
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
        diagnostics
    }

    /// Analyze the SQL in a Python file: the string literals passed to calls of
    /// `functions` (see [`crate::python`]). Diagnostics point at the query's
    /// location in `source`.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::python::DEFAULT_FUNCTIONS;
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let source = r#"cur.execute("SELECT nme FROM users WHERE id = %s", (user_id,))"#;
    /// let diagnostics = Analyzer::new(&catalog).analyze_python(source, DEFAULT_FUNCTIONS);
    /// assert_eq!(diagnostics.len(), 1);
    /// assert_eq!(diagnostics[0].span.unwrap().column, 21);
    /// ```
    pub fn analyze_python<S: AsRef<str>>(
        &mut self,
        source: &str,
        functions: &[S],
    ) -> Vec<Diagnostic> {
        let (source, bom) = strip_bom(source);
        let extracted = crate::python::extract(source, functions, self.dialect);
        let mut diagnostics = self.analyze_text(
            &extracted.text,
            source,
            &extracted.identifiers,
            &extracted.fragments,
        );
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
        diagnostics
    }

    /// Analyze the SQL in a Go file: the string literals passed to calls of
    /// `functions` (see [`crate::go`]). Diagnostics point at the query's location
    /// in `source`.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::go::DEFAULT_FUNCTIONS;
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id INTEGER, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let source = "row := db.QueryRowContext(ctx, \"SELECT nme FROM users WHERE id = $1\", id)";
    /// let diagnostics = Analyzer::new(&catalog).analyze_go(source, DEFAULT_FUNCTIONS);
    /// assert_eq!(diagnostics.len(), 1);
    /// assert_eq!(diagnostics[0].span.unwrap().column, 40);
    /// ```
    pub fn analyze_go<S: AsRef<str>>(&mut self, source: &str, functions: &[S]) -> Vec<Diagnostic> {
        let (source, bom) = strip_bom(source);
        let extracted = crate::go::extract(source, functions, self.dialect);
        let mut diagnostics = self.analyze_text(
            &extracted.text,
            source,
            &extracted.identifiers,
            &extracted.fragments,
        );
        shift_offsets(&mut diagnostics, bom);
        self.shift_references(bom);
        diagnostics
    }

    /// Describe what each statement of `sql` returns: its output columns and their
    /// types, as far as the schema tells (see [`QueryDescription`]). The file is
    /// analyzed as [`Analyzer::analyze`] does, and its diagnostics are returned too.
    ///
    /// # Example
    ///
    /// ```
    /// use sqlsift_core::analyzer::Analyzer;
    /// use sqlsift_core::schema::SchemaBuilder;
    ///
    /// let mut builder = SchemaBuilder::new();
    /// builder.parse("CREATE TABLE users (id BIGINT, name TEXT);").unwrap();
    /// let (catalog, _) = builder.build();
    ///
    /// let description = Analyzer::new(&catalog).describe("SELECT id, name AS n FROM users");
    /// let columns = description.queries[0].columns.as_ref().unwrap();
    /// assert_eq!(columns[1].name.as_deref(), Some("n"));
    /// assert_eq!(columns[0].ty.as_ref().unwrap().display_name(), "bigint");
    /// ```
    pub fn describe(&mut self, sql: &str) -> Description {
        self.describing(|analyzer| analyzer.analyze(sql))
    }

    /// [`Analyzer::describe`] for the SQL in a TypeScript or JavaScript file (see
    /// [`Analyzer::analyze_embedded`])
    pub fn describe_embedded<S: AsRef<str>>(&mut self, source: &str, tags: &[S]) -> Description {
        self.describing(|analyzer| analyzer.analyze_embedded(source, tags))
    }

    /// [`Analyzer::describe`] for the SQL in a Vue or Svelte component (see
    /// [`Analyzer::analyze_embedded_component`])
    pub fn describe_embedded_component<S: AsRef<str>>(
        &mut self,
        source: &str,
        tags: &[S],
    ) -> Description {
        self.describing(|analyzer| analyzer.analyze_embedded_component(source, tags))
    }

    /// [`Analyzer::describe`] for the SQL in a Rust file (see
    /// [`Analyzer::analyze_rust`])
    pub fn describe_rust<S: AsRef<str>>(&mut self, source: &str, macros: &[S]) -> Description {
        self.describing(|analyzer| analyzer.analyze_rust(source, macros))
    }

    /// [`Analyzer::describe`] for the SQL in a Python file (see
    /// [`Analyzer::analyze_python`])
    pub fn describe_python<S: AsRef<str>>(&mut self, source: &str, functions: &[S]) -> Description {
        self.describing(|analyzer| analyzer.analyze_python(source, functions))
    }

    /// [`Analyzer::describe`] for the SQL in a Go file (see
    /// [`Analyzer::analyze_go`])
    pub fn describe_go<S: AsRef<str>>(&mut self, source: &str, functions: &[S]) -> Description {
        self.describing(|analyzer| analyzer.analyze_go(source, functions))
    }

    /// Run `analyze`, collecting the statements it describes
    fn describing(&mut self, analyze: impl FnOnce(&mut Self) -> Vec<Diagnostic>) -> Description {
        self.descriptions = Some(Vec::new());
        let diagnostics = analyze(self);
        Description {
            queries: self.descriptions.take().unwrap_or_default(),
            diagnostics,
        }
    }

    /// Analyze `sql`, which has the same lines and character columns as `original`
    /// (where byte offsets point). Name diagnostics on the `substituted` identifiers
    /// ((line, first column, end column)) are dropped. A statement with one of the
    /// `fragments` ((line, column, whether it may join relations) of a dropped query
    /// fragment) may have clauses, and relations, the text doesn't show.
    fn analyze_text(
        &mut self,
        sql: &str,
        original: &str,
        substituted: &[(usize, usize, usize)],
        fragments: &[(usize, usize, bool)],
    ) -> Vec<Diagnostic> {
        self.diagnostics.clear();
        if let Some(descriptions) = &mut self.descriptions {
            descriptions.clear();
        }
        if let Some(references) = &mut self.references {
            references.clear();
        }

        // Template tags, then psql meta-commands and variables (the rewrites keep
        // every line and character column)
        let catalog = self.catalog;
        let template = match self.templating {
            Templating::Jinja => templating::mask_jinja(
                sql,
                &|schema, table| {
                    let name = QualifiedName::with_schema(schema, table);
                    catalog.table_exists(&name) || catalog.view_exists(&name)
                },
                templating::dbt_adapter(self.dialect),
            ),
            Templating::None => Template::unchanged(sql),
        };
        // aiosql / HugSQL parameters (`:id`, `:v*:ids`, `:i:col`) become placeholders
        let named = named_queries::mask(&template.masked.text, self.dialect);
        let fragments: Vec<(usize, usize, bool)> =
            fragments.iter().chain(&named.fragments).copied().collect();
        let source = match self.dialect {
            dialect if dialect.is_psql() => psql::preprocess(&named.text),
            SqlDialect::MySQL => mysql::preprocess(&named.text),
            _ => Preprocessed::unchanged(&named.text),
        };
        // sqlc parameters (`sqlc.arg(name)`, `@name`) become placeholders
        let text = sqlc::mask_parameters(&source.text, self.dialect);
        // BigQuery syntax sqlparser doesn't parse, rewritten (diagnostics are moved
        // back over the characters it inserts below)
        let rewritten = (self.dialect == SqlDialect::BigQuery)
            .then(|| bigquery::rewrite_query(&text))
            .flatten();
        let text: &str = rewritten.as_ref().map_or(&text, |r| r.text.as_str());
        // A file that looks like a template but isn't masked doesn't parse
        let template_hint =
            self.templating == Templating::None && templating::looks_like_jinja(sql);

        // Parse inline disable directives from comments (`{# ... #}` comments too)
        let directives = InlineDirectives::parse(&template.directive_text());
        // Query names (sqlc `-- name: GetPost :one`, aiosql, HugSQL)
        let query_names = QueryNames::parse(sql);

        // Parse the SQL
        let lines = LineIndex::new(text);
        let arena = syntax::Arena::new();
        let (statements, mut skipped) = self.parse_statements(text, &lines, &arena);
        // Applied in source order, before the statements that follow them
        skipped.reverse();

        // Tables, views and types created, altered or dropped by the file's own
        // statements, applied to a copy of the catalog made on the first such
        // statement, so they are visible to the later statements of this file only
        let mut file_schema: Option<SchemaBuilder> = None;
        // Script variables declared so far (`DECLARE x INT64 DEFAULT 0`)
        let mut variables: Vec<String> = Vec::new();
        // Placeholders of each described statement
        let mut placeholders: Vec<Vec<(Span, Option<parameters::ParameterType>)>> = Vec::new();

        // Where each statement's text starts in the template source (to tell whether
        // it has template tags)
        let source_lines = LineIndex::new(sql);
        let starts: Vec<usize> = statements
            .iter()
            .map(|(_, o, _)| source_lines.byte_offset(o.line as u64, o.column as u64))
            .collect();

        // Analyze each statement
        for (index, (stmt, origin, syntax)) in statements.iter().enumerate() {
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
            let restrictions = [
                DiagnosticKind::UnfilteredWrite,
                DiagnosticKind::InsertWithoutColumns,
                DiagnosticKind::SelectStar,
                DiagnosticKind::LimitWithoutOrderBy,
            ]
            .into_iter()
            .any(|kind| self.rules.level(kind) != crate::rules::RuleLevel::Off);
            let templated_name = |span: &Span| {
                let mut span = *span;
                origin.shift(&mut span);
                template.masked.is_substituted(span.line, span.column)
            };
            // Template tags may expand to columns or whole clauses
            let templated = self.templating != Templating::None && {
                let end = starts.get(index + 1).copied().unwrap_or(sql.len());
                let text = sql.get(starts[index]..end).unwrap_or_default();
                text.contains("{{") || text.contains("{%")
            };
            // Query fragments interpolated between clauses may join relations
            let next = statements
                .get(index + 1)
                .map(|(_, o, _)| (o.line, o.column));
            let statement_fragments: Vec<bool> = fragments
                .iter()
                .filter(|&&(line, column, _)| {
                    (line, column) >= (origin.line, origin.column)
                        && next.map_or(true, |next| (line, column) < next)
                })
                .map(|&(_, _, joins)| joins)
                .collect();
            let mut resolver = Resolver::new(catalog, self.dialect)
                .with_variables(&variables)
                .with_templated_names(&templated_name);
            resolver.templated_statement = templated || !statement_fragments.is_empty();
            resolver.hidden_relations = statement_fragments.contains(&true);
            // The restriction rules read the tree of a statement the in-house parser read
            resolver.restrictions = restrictions && syntax.is_none();
            #[cfg(feature = "inhouse-parser")]
            if let (true, Some(node)) = (restrictions, syntax) {
                resolver
                    .diagnostics
                    .extend(syntax::restrictions(text, &lines, node));
            }
            resolver.references = self.references.is_some().then(Vec::new);
            let describing = self.descriptions.is_some() && QueryKind::of(stmt).is_some();
            if describing {
                resolver.parameter_types = Some(parameters::ParameterTypes::default());
            }
            if resolver.restrictions {
                resolver.check_unordered_limits(stmt);
            }
            let output = resolver.statement(stmt);
            if let (Some(all), Some(found)) = (&mut self.references, resolver.references.take()) {
                all.extend(found.into_iter().map(|mut reference| {
                    origin.shift(&mut reference.span);
                    reference
                }));
            }
            if let Some(descriptions) = &mut self.descriptions {
                if let Some(kind) = QueryKind::of(stmt) {
                    // Placeholders, located relative to the input, with their types
                    let types = resolver.parameter_types.take().unwrap_or_default();
                    placeholders.push(
                        describe::placeholder_locations(stmt)
                            .into_iter()
                            .map(|(line, column)| {
                                let ty = types.get((line, column)).cloned();
                                let mut span = Span::with_location(line, column, 1);
                                origin.shift(&mut span);
                                (span, ty)
                            })
                            .collect(),
                    );
                    // Where the statement starts (its first token), relative to the input
                    let start = stmt.span().start;
                    let mut span = if start.line > 0 {
                        Span::with_location(start.line as usize, start.column as usize, 1)
                    } else {
                        Span::with_location(1, 1, 1)
                    };
                    origin.shift(&mut span);
                    // A template may add columns (`{{ dbt_utils.star(...) }}`, a loop)
                    let columns = if resolver.templated_statement {
                        None
                    } else if describe::has_returning(stmt) {
                        output.map(|columns| {
                            columns
                                .into_iter()
                                .map(|c| OutputColumn::new(c, self.dialect))
                                .collect()
                        })
                    } else {
                        Some(Vec::new())
                    };
                    descriptions.push(QueryDescription {
                        line: span.line,
                        column: span.column,
                        name: None,
                        kind,
                        columns,
                        parameters: Vec::new(),
                    });
                }
            }
            if let Statement::Declare { stmts } = stmt {
                variables.extend(
                    stmts
                        .iter()
                        .flat_map(|d| d.names.iter().map(|n| n.value.clone())),
                );
            }

            // Locations relative to the input
            for mut diagnostic in resolver.into_diagnostics() {
                if let Some(span) = diagnostic.span.as_mut() {
                    origin.shift(span);
                }
                self.diagnostics.push(diagnostic);
            }

            if SchemaBuilder::changes_schema(stmt) {
                file_schema
                    .get_or_insert_with(|| {
                        SchemaBuilder::from_catalog(self.catalog.clone(), self.dialect)
                    })
                    .apply_statement(stmt, origin.line);
            }
        }

        if let Some(descriptions) = &mut self.descriptions {
            let original_text = LineIndex::new(original);
            for (description, placeholders) in descriptions.iter_mut().zip(placeholders) {
                // A placeholder's original text names its parameter (`:id` became `$1`)
                let occurrences: Vec<(&str, Option<parameters::ParameterType>)> = placeholders
                    .into_iter()
                    .map(|(mut span, ty)| {
                        if let Some(rewritten) = &rewritten {
                            rewritten.map_span(&mut span);
                        }
                        let offset =
                            original_text.byte_offset(span.line as u64, span.column as u64);
                        (original.get(offset..).unwrap_or_default(), ty)
                    })
                    .collect();
                description.parameters = describe::parameters(occurrences);
                if let Some(rewritten) = &rewritten {
                    let mut span = Span::with_location(description.line, description.column, 1);
                    rewritten.map_span(&mut span);
                    description.line = span.line;
                    description.column = span.column;
                }
                description.name = query_names.at(description.line).map(str::to_string);
            }
        }

        if let Some(rewritten) = &rewritten {
            for reference in self.references.iter_mut().flatten() {
                rewritten.map_span(&mut reference.span);
            }
            for diagnostic in &mut self.diagnostics {
                let labels = diagnostic.labels.iter_mut().map(|l| &mut l.span);
                for span in diagnostic.span.iter_mut().chain(labels) {
                    rewritten.map_span(span);
                }
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
            || named.text.len() != sql.len()
            || source.text.len() != sql.len()
            || rewritten.is_some())
        .then(|| LineIndex::new(original));
        let offsets = original_lines.as_ref().unwrap_or(&lines);
        for diagnostic in &mut self.diagnostics {
            let labels = diagnostic.labels.iter_mut().map(|l| &mut l.span);
            for span in diagnostic.span.iter_mut().chain(labels) {
                offsets.fill_offset(span);
            }
        }
        for reference in self.references.iter_mut().flatten() {
            offsets.fill_offset(&mut reference.span);
        }
        if let Some(references) = &mut self.references {
            references.sort_by_key(|r| (r.span.line, r.span.column));
            references.dedup();
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
                // (`FROM ${tbl}`) is unknown; the `SELECT*` a template literal
                // became is not the query's
                let substituted = matches!(
                    d.kind,
                    DiagnosticKind::TableNotFound
                        | DiagnosticKind::ColumnNotFound
                        | DiagnosticKind::AmbiguousColumn
                        | DiagnosticKind::SelectStar
                ) && (source.is_substituted(span.line, span.column)
                    || template.masked.is_substituted(span.line, span.column)
                    || psql::is_within(substituted, span.line, span.column)
                    || psql::is_within(&named.identifiers, span.line, span.column))
                    || is_templated_alias(&d, &template)
                    || (d.kind == DiagnosticKind::TableNotFound
                        && template.may_be_templated_cte(span.line, span.column));
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
    ///
    /// With the in-house parser ([`crate::parser_hook`]), PostgreSQL text is read by it
    /// into `arena`, and the statements it read come with their tree.
    fn parse_statements<'t>(
        &mut self,
        sql: &str,
        lines: &LineIndex,
        arena: &'t syntax::Arena,
    ) -> ParsedStatements<'t> {
        #[cfg(feature = "inhouse-parser")]
        if self.dialect == SqlDialect::PostgreSQL && crate::parser_hook::enabled() {
            return self.parse_statements_inhouse(sql, lines, arena);
        }
        #[cfg(not(feature = "inhouse-parser"))]
        let _ = arena;
        let dialect = self.dialect.parser_dialect();
        let error = match Parser::parse_sql(dialect.as_ref(), sql) {
            Ok(statements) => {
                let statements = statements
                    .into_iter()
                    .map(|s| (analyzed(s), Origin::START, None))
                    .collect();
                return (statements, Vec::new());
            }
            Err(error) => error,
        };

        let Some(mut ranges) = statement_ranges(dialect.as_ref(), sql, lines) else {
            // Tokenizer error: nothing can be parsed reliably
            self.diagnostics
                .push(parse_error_diagnostic(&error, sql, 0..sql.len(), lines));
            return (Vec::new(), Vec::new());
        };
        // Snowflake Scripting blocks are skipped (their statements end with `;` too)
        if self.dialect == SqlDialect::Snowflake {
            let blocks = snowflake::scripting_blocks(dialect.as_ref(), sql, &|location| {
                lines.byte_offset(location.line, location.column)
            });
            ranges.retain(|r| !blocks.iter().any(|b| b.start < r.end && r.start < b.end));
        }

        let mut statements = Vec::new();
        let mut skipped = Vec::new();
        for range in ranges {
            self.parse_range(sql, range, lines, &mut statements, &mut skipped, true);
        }
        (statements, skipped)
    }

    /// Parses one statement's text (`sql[range]`) on its own with sqlparser, retrying
    /// without clauses it doesn't parse; see [`Self::parse_statements`]. A syntax error
    /// is reported if `report_error`.
    fn parse_range(
        &mut self,
        sql: &str,
        range: Range<usize>,
        lines: &LineIndex,
        statements: &mut Vec<(Statement, Origin, syntax::Syntax<'_>)>,
        skipped: &mut Vec<(Origin, SkippedDefinition)>,
        report_error: bool,
    ) {
        let dialect = self.dialect.parser_dialect();
        let (line, column) = lines.line_column(range.start);
        let origin = Origin { line, column };
        let text = &sql[range.clone()];
        let error = match Parser::parse_sql(dialect.as_ref(), text) {
            Ok(parsed) => {
                statements.extend(parsed.into_iter().map(|s| (analyzed(s), origin, None)));
                return;
            }
            Err(error) => error,
        };
        let retried = mask_unsupported_clauses(self.dialect, text, false)
            .and_then(|(masked, _)| Parser::parse_sql(dialect.as_ref(), &masked).ok())
            .or_else(|| match self.dialect {
                // `t.$1` positional columns
                SqlDialect::Snowflake => snowflake::mask_positional_columns(dialect.as_ref(), text)
                    .and_then(|masked| Parser::parse_sql(dialect.as_ref(), &masked).ok()),
                // `FOR NO KEY UPDATE` / `FOR KEY SHARE`
                dialect_kind if dialect_kind.is_psql() => {
                    mask_row_lock_strength(dialect.as_ref(), text)
                        .and_then(|masked| Parser::parse_sql(dialect.as_ref(), &masked).ok())
                }
                _ => None,
            });
        if let Some(parsed) = retried {
            statements.extend(parsed.into_iter().map(|s| (analyzed(s), origin, None)));
            return;
        }
        // PostgreSQL maintenance and procedural statements (ANALYZE, VACUUM, REFRESH MATERIALIZED VIEW, DO):
        // statistics or procedural blocks only, nothing to check
        if self.dialect.is_psql() && is_skipped_statement(dialect.as_ref(), text) {
            return;
        }
        // `CREATE EXTENSION ... VERSION '...'` (Supabase/pgTAP test files): loads
        // functions and types only, no relation to check
        if self.dialect.is_psql()
            && starts_with_words(dialect.as_ref(), text, &["CREATE", "EXTENSION"])
        {
            return;
        }
        // Snowflake administration statements (ALTER SESSION, CREATE WAREHOUSE, ...)
        if self.dialect == SqlDialect::Snowflake
            && snowflake::is_unanalyzed_statement(dialect.as_ref(), text)
        {
            return;
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
        if report_error {
            self.diagnostics
                .push(parse_error_diagnostic(&error, sql, range, lines));
        }
    }

    /// [`Self::parse_statements`] with the in-house parser: the statements it lowers to
    /// the analyzer's AST with their tree, sqlparser for the ones it leaves to it.
    #[cfg(feature = "inhouse-parser")]
    fn parse_statements_inhouse<'t>(
        &mut self,
        sql: &str,
        lines: &LineIndex,
        arena: &'t syntax::Arena,
    ) -> ParsedStatements<'t> {
        use sqlsift_parser::analysis::Outcome;
        let mut statements = Vec::new();
        let mut skipped = Vec::new();
        for item in sqlsift_parser::analysis::statements(arena, sql) {
            let range = item.range.to_range();
            match item.outcome {
                Outcome::Lowered(statement) => {
                    statements.push((*statement, Origin::START, item.node));
                }
                Outcome::Unsupported(_) => {
                    // Valid PostgreSQL: a statement sqlparser can't parse isn't an error
                    self.parse_range(sql, range, lines, &mut statements, &mut skipped, false);
                }
                Outcome::SyntaxError(error) => {
                    // Located the way sqlparser locates its errors: in the statement's text
                    let message = error.message;
                    let offset = error.range.start as usize;
                    let text = &sql[range.start..];
                    let (line, column) = crate::parser_hook::line_column(
                        text,
                        offset.saturating_sub(range.start).min(text.len()),
                    );
                    let error = ParserError::ParserError(format!(
                        "{message} at Line: {line}, Column: {column}"
                    ));
                    self.diagnostics
                        .push(parse_error_diagnostic(&error, sql, range, lines));
                }
            }
        }
        (statements, skipped)
    }
}

/// A statement sqlparser parsed, as the analyzer reads it: MySQL's `INSERT ... SET` as
/// `INSERT ... VALUES`, converted to the analyzer's AST
fn analyzed(mut statement: sqlparser::ast::Statement) -> Statement {
    mysql::insert_set_as_values(&mut statement);
    crate::convert::statement(&statement)
}

/// Significant tokens of `text` with their byte ranges (`None` if it can't be
/// tokenized)
fn significant_tokens(dialect: &dyn Dialect, text: &str) -> Option<Vec<(Token, Range<usize>)>> {
    let lines = LineIndex::new(text);
    let tokens = Tokenizer::new(dialect, text)
        .tokenize_with_location()
        .ok()?;
    Some(
        tokens
            .into_iter()
            .filter(|t| !matches!(t.token, Token::Whitespace(_)))
            .map(|t| {
                let start = lines.byte_offset(t.span.start.line, t.span.start.column);
                let end = lines.byte_offset(t.span.end.line, t.span.end.column);
                (t.token, start..end)
            })
            .collect(),
    )
}

/// Whether an unparseable PostgreSQL statement is a maintenance or procedural command
/// that has no relations or schema elements to check (ANALYZE, VACUUM, REFRESH MATERIALIZED VIEW, DO)
fn is_skipped_statement(dialect: &dyn Dialect, text: &str) -> bool {
    first_word_is(dialect, text, "ANALYZE")
        || first_word_is(dialect, text, "VACUUM")
        || first_word_is(dialect, text, "DO")
        || starts_with_words(dialect, text, &["REFRESH", "MATERIALIZED", "VIEW"])
}

/// Whether the first word of `text` (after comments) is the keyword `keyword`
fn first_word_is(dialect: &dyn Dialect, text: &str, keyword: &str) -> bool {
    starts_with_words(dialect, text, &[keyword])
}

/// Whether `text` (after comments) starts with the keywords `keywords`
fn starts_with_words(dialect: &dyn Dialect, text: &str, keywords: &[&str]) -> bool {
    significant_tokens(dialect, text).is_some_and(|tokens| {
        tokens.len() >= keywords.len()
            && tokens.iter().zip(keywords).all(|((token, _), keyword)| {
                matches!(token, Token::Word(w)
                    if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(keyword))
            })
    })
}

/// `text` with the row-level lock strengths sqlparser doesn't parse blanked to the
/// ones it does: `FOR NO KEY UPDATE` becomes `FOR UPDATE` and `FOR KEY SHARE`
/// becomes `FOR SHARE` (`None` if there are none)
fn mask_row_lock_strength(dialect: &dyn Dialect, text: &str) -> Option<String> {
    let tokens = significant_tokens(dialect, text)?;
    let is = |i: usize, kw: &str| {
        matches!(tokens.get(i), Some((Token::Word(w), _))
            if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(kw))
    };
    let mut blank = Vec::new();
    for i in 0..tokens.len() {
        if !is(i, "FOR") {
            continue;
        }
        if is(i + 1, "NO") && is(i + 2, "KEY") && is(i + 3, "UPDATE") {
            blank.push(tokens[i + 1].1.start..tokens[i + 2].1.end);
        } else if is(i + 1, "KEY") && is(i + 2, "SHARE") {
            blank.push(tokens[i + 1].1.clone());
        }
    }
    if blank.is_empty() {
        return None;
    }
    let mut masked = text.to_string();
    for range in blank {
        masked.replace_range(range.clone(), &" ".repeat(range.len()));
    }
    Some(masked)
}

/// Whether `d` reports a qualifier missing from FROM (`a_o.v`) whose name may be an
/// alias the template builds from tags (`left join o as {{ f }}_o` in a loop)
fn is_templated_alias(d: &Diagnostic, template: &Template) -> bool {
    if d.kind != DiagnosticKind::TableNotFound {
        return false;
    }
    let Some(name) = d
        .message
        .strip_prefix("Table or alias '")
        .and_then(|m| m.strip_suffix("' not found in FROM clause"))
    else {
        return false;
    };
    let name = name.rsplit('.').next().unwrap_or(name);
    template.may_be_templated_alias(name)
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
/// each with the position where its text starts (and the tree of a statement the
/// in-house parser read)
type ParsedStatements<'t> = (
    Vec<(Statement, Origin, syntax::Syntax<'t>)>,
    Vec<(Origin, SkippedDefinition)>,
);

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
    query: &sqlsift_ast::Query,
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
