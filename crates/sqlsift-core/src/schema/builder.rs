//! Schema builder - converts SQL AST to Catalog

use sqlparser::ast::{
    AlterColumnOperation, AlterTableOperation, AlterType, AlterTypeAddValuePosition,
    AlterTypeOperation, ColumnOption, ColumnOptionDef, CreateTableLikeKind, DataType, Expr, Ident,
    IndexColumn, ObjectName, ObjectType, Query, RenameTableNameKind, Set, SetExpr, Statement,
    TableConstraint, TriggerEvent, TriggerPeriod, Use, UserDefinedTypeRepresentation, Value,
    ValueWithSpan,
};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use crate::analyzer;
use crate::ast_ext::{name_last, ObjectNamePartExt};
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::psql;
use crate::schema::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, IdentityKind,
    PrimaryKeyDef, QualifiedName, SkippedDefinition, SourceLocation, TableDef, UniqueConstraintDef,
    ViewDef,
};
use crate::types::SqlType;

/// Builder for constructing a Catalog from SQL schema definitions
pub struct SchemaBuilder {
    catalog: Catalog,
    diagnostics: Vec<Diagnostic>,
    dialect: SqlDialect,
    /// Applying the statements of a query file: a `CREATE TABLE ... AS` whose columns
    /// can't be inferred defines a relation with unknown columns (so references to
    /// them aren't reported) instead of warning
    query_file: bool,
    /// The schema file being parsed ([`Self::parse_file`]), named in the hints
    /// on renamed and dropped tables and columns
    source_file: Option<String>,
    /// Lines of the input before the text being parsed (statement locations are
    /// relative to that text)
    line_offset: usize,
    /// Column of the input where the text being parsed starts
    column_offset: usize,
    /// Line of the statement being applied, for nodes without a location
    statement_line: Option<usize>,
}

impl SchemaBuilder {
    pub fn new() -> Self {
        Self::with_dialect(SqlDialect::default())
    }

    pub fn with_dialect(dialect: SqlDialect) -> Self {
        let mut catalog = Catalog::new();
        catalog.case_sensitive_names = dialect == SqlDialect::PostgreSQL;
        catalog.strict_schemas = dialect == SqlDialect::PostgreSQL;
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect,
            query_file: false,
            source_file: None,
            line_offset: 0,
            column_offset: 1,
            statement_line: None,
        }
    }

    /// Continue building from an existing catalog: applies the DDL statements of a
    /// query file to a file-local copy of the schema
    pub(crate) fn from_catalog(catalog: Catalog, dialect: SqlDialect) -> Self {
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect,
            query_file: true,
            source_file: None,
            line_offset: 0,
            column_offset: 1,
            statement_line: None,
        }
    }

    /// Apply one parsed statement of the query file, which starts at line
    /// `line` of the file, to the catalog (statements that don't define or
    /// change the schema are ignored)
    pub(crate) fn apply_statement(&mut self, stmt: &Statement, line: usize) {
        self.line_offset = line.saturating_sub(1);
        self.statement_line = Some(line);
        self.process_statement(stmt);
        self.line_offset = 0;
        self.statement_line = None;
    }

    /// Record a definition in the query file that could not be parsed (for
    /// diagnostics on the later statements that use it)
    pub(crate) fn skip_definition(&mut self, definition: SkippedDefinition) {
        self.catalog.skipped_definitions.push(definition);
    }

    /// Whether `stmt` is a statement [`SchemaBuilder`] applies to the catalog
    pub(crate) fn changes_schema(stmt: &Statement) -> bool {
        match stmt {
            Statement::CreateTable(_)
            | Statement::CreateIndex(_)
            | Statement::CreateType { .. }
            | Statement::CreateView { .. }
            | Statement::AlterTable { .. }
            | Statement::AlterType(_)
            | Statement::Drop {
                object_type:
                    ObjectType::Table
                    | ObjectType::View
                    | ObjectType::MaterializedView
                    | ObjectType::Type,
                ..
            } => true,
            Statement::Query(query) => select_into(query).is_some(),
            Statement::Set(set) => search_path_values(set).is_some(),
            Statement::Use(use_stmt) => used_schema(use_stmt).is_some(),
            _ => false,
        }
    }

    /// Parse the SQL schema definitions of the file `path` (as it should be shown
    /// to the user), like [`Self::parse`]. Diagnostics on queries that use a table
    /// or column the file renamed or dropped name the file and line.
    ///
    /// # Errors
    ///
    /// The diagnostics, when there are errors (see [`Self::parse`]).
    pub fn parse_file(&mut self, sql: &str, path: &str) -> Result<(), Vec<Diagnostic>> {
        self.source_file = Some(path.to_string());
        let first_new = self.diagnostics.len();
        let result = self.parse(sql);
        self.source_file = None;
        // Schema warnings are listed without the schema text: name the file and line
        for diagnostic in self.diagnostics.iter_mut().skip(first_new) {
            if diagnostic.severity == crate::error::Severity::Warning {
                diagnostic.message = match diagnostic.span.filter(|s| s.line > 0) {
                    Some(span) => format!("{path}:{}: {}", span.line, diagnostic.message),
                    None => format!("{path}: {}", diagnostic.message),
                };
            }
        }
        result
    }

    /// Parse SQL schema definitions and build the catalog.
    ///
    /// dbmate `-- migrate:down` sections are ignored (see
    /// [`strip_down_migrations`](crate::schema::strip_down_migrations)).
    pub fn parse(&mut self, sql: &str) -> Result<(), Vec<Diagnostic>> {
        // A leading byte order mark is not part of the SQL
        let (sql, bom) = crate::analyzer::strip_bom(sql);
        let first_new = self.diagnostics.len();
        self.parse_text(sql);
        // Spans made from a line and column (no byte offset yet)
        for span in self.diagnostics[first_new..]
            .iter_mut()
            .filter_map(|d| d.span.as_mut())
            .filter(|s| s.offset == 0 && s.line > 0)
        {
            span.offset = byte_offset_of(sql, span.line, span.column).unwrap_or(0);
        }
        crate::analyzer::shift_offsets(&mut self.diagnostics[first_new..], bom);

        if self
            .diagnostics
            .iter()
            .any(|d| d.severity == crate::error::Severity::Error)
        {
            Err(std::mem::take(&mut self.diagnostics))
        } else {
            Ok(())
        }
    }

    /// Record the names of functions, aggregates and operators `sql` creates (in
    /// any statement, parsed or not), which may overload built-in ones
    fn record_user_routines(&mut self, sql: &str) {
        let lower = sql.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '"' | '$')))
            .filter(|w| !w.is_empty())
            .collect();
        for (i, word) in words.iter().enumerate() {
            if *word != "create" {
                continue;
            }
            let mut rest = words[i + 1..].iter();
            let mut kind = rest.next();
            if kind == Some(&"or") {
                rest.next(); // replace
                kind = rest.next();
            }
            match kind {
                Some(&("function" | "aggregate")) => {
                    let name = rest
                        .next()
                        .and_then(|n| n.rsplit('.').next())
                        .map(|n| n.trim_matches('"').to_string());
                    if let Some(name) = name {
                        if !self.catalog.user_functions.contains(&name) {
                            self.catalog.user_functions.push(name);
                        }
                    }
                }
                Some(&"operator") => self.catalog.user_operators = true,
                _ => {}
            }
        }
    }

    /// Build the catalog from the statements of `sql`
    fn parse_text(&mut self, sql: &str) {
        let sql = &*crate::schema::strip_down_migrations(sql);
        self.record_user_routines(sql);
        let dialect = self.dialect.parser_dialect();

        // psql meta-commands in dumps and scripts (`\connect`, `\restrict`, `\i`, ...)
        let source = match self.dialect {
            dialect if dialect.is_psql() => psql::preprocess(sql),
            SqlDialect::MySQL => crate::mysql::preprocess(sql),
            _ => psql::Preprocessed::unchanged(sql),
        };
        let sql: &str = &source.text;

        // A `SET search_path` applies to the rest of its file only
        let search_path = self.catalog.search_path.clone();

        // Try parsing the entire SQL first (fast path)
        match crate::stack::parse_sql(dialect.as_ref(), sql) {
            Ok(statements) => {
                for stmt in statements {
                    self.process_statement(&stmt);
                }
            }
            Err(_) => {
                // Fall back to statement-by-statement parsing to skip unsupported syntax
                self.parse_statements_individually(sql);
            }
        }
        self.catalog.search_path = search_path;
    }

    /// Parse SQL statements individually, skipping those that fail to parse.
    /// This allows sqlsift to handle schema files containing unsupported syntax
    /// (e.g., CREATE FUNCTION, CREATE TRIGGER, CREATE DOMAIN) by gracefully
    /// skipping unparseable statements while still processing the rest.
    ///
    /// Skipped statements that define tables, views or types produce a warning.
    fn parse_statements_individually(&mut self, sql: &str) {
        let dialect = self.dialect.parser_dialect();

        for raw_stmt in split_sql_statements(sql, self.dialect) {
            if raw_stmt.trim().is_empty() {
                continue;
            }

            // Parse the untrimmed text so parser locations are relative to `raw_stmt`
            let parsed = crate::stack::parse_sql(dialect.as_ref(), raw_stmt).or_else(|err| {
                // BigQuery: retry with nested type parameter lists closed by `> >`
                // (sqlparser rejects some `>>`); only locations past them shift
                match self.dialect {
                    SqlDialect::BigQuery => {
                        crate::analyzer::bigquery::split_closing_angle_brackets(raw_stmt)
                            .and_then(|split| {
                                crate::stack::parse_sql(dialect.as_ref(), &split).ok()
                            })
                            .ok_or(err)
                    }
                    _ => Err(err),
                }
            });
            match parsed {
                Ok(stmts) => {
                    // `raw_stmt` is a subslice of `sql`
                    let offset = raw_stmt.as_ptr() as usize - sql.as_ptr() as usize;
                    let (line, column) = line_column_at(sql, offset);
                    self.line_offset = line - 1;
                    self.column_offset = column;
                    for stmt in stmts {
                        self.process_statement(&stmt);
                    }
                    self.line_offset = 0;
                    self.column_offset = 1;
                }
                Err(err) => {
                    // `raw_stmt` is a subslice of `sql`
                    let offset = raw_stmt.as_ptr() as usize - sql.as_ptr() as usize;
                    self.process_unparsed_statement(sql, offset, raw_stmt, &err);
                }
            }
        }
    }

    /// Handle a statement sqlparser could not parse: apply the few forms we
    /// understand from tokens (ALTER TYPE, DROP MATERIALIZED VIEW, LIKE with
    /// INCLUDING options), warn about skipped table/view/type definitions, and
    /// silently skip everything else (functions, triggers, GRANT, ...).
    fn process_unparsed_statement(
        &mut self,
        sql: &str,
        offset: usize,
        stmt: &str,
        err: &ParserError,
    ) {
        let parser_dialect = self.dialect.parser_dialect();
        let tokenize = |text: &str| {
            Tokenizer::new(parser_dialect.as_ref(), text)
                .with_unescape(false)
                .tokenize_with_location()
        };
        // If the statement can't be tokenized, the text before the error still tells
        // what it defines (so a skipped definition is reported)
        let mut text = stmt;
        let tokens = loop {
            match tokenize(text) {
                Ok(tokens) => break tokens,
                Err(error) => {
                    let end = byte_offset_of(
                        text,
                        usize::try_from(error.location.line).unwrap_or(0),
                        usize::try_from(error.location.column).unwrap_or(0),
                    )
                    .unwrap_or(0);
                    if end == 0 || end >= text.len() {
                        return;
                    }
                    text = &text[..end];
                }
            }
        };
        let significant: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t.token, Token::Whitespace(_)))
            .collect();
        let Some(first) = significant.first() else {
            return;
        };
        let words: Vec<String> = significant
            .iter()
            .map(|t| match &t.token {
                Token::Word(w) if w.quote_style.is_none() => w.value.to_uppercase(),
                _ => String::new(),
            })
            .collect();
        let word = |i: usize| words.get(i).map_or("", String::as_str);
        let sig_tokens: Vec<&Token> = significant.iter().map(|t| &t.token).collect();

        if word(0) == "ALTER" && word(1) == "TYPE" {
            self.process_alter_type_tokens(&sig_tokens[2..]);
            return;
        }
        if word(0) == "DROP" && word(1) == "MATERIALIZED" && word(2) == "VIEW" {
            let mut rest = &sig_tokens[3..];
            if words.get(3).map(String::as_str) == Some("IF") {
                rest = rest.get(2..).unwrap_or(&[]);
            }
            for name in split_object_names(rest) {
                let name = self.catalog.qualified_name(&name);
                self.catalog.drop_view(&name);
            }
            return;
        }

        // A trigger sqlparser doesn't parse (MySQL's `FOR EACH ROW SET NEW.x = ...`,
        // PostgreSQL's `WHEN (...)` forms): `[BEFORE | INSTEAD OF] events ON table`
        if word(0) == "CREATE" {
            let trigger = (1..words.len().min(12))
                .take_while(|&i| !matches!(word(i), "FUNCTION" | "PROCEDURE" | "RETURNS"))
                .find(|&i| word(i) == "TRIGGER");
            if let Some(trigger) = trigger {
                let on = (trigger + 1..words.len()).find(|&i| word(i) == "ON");
                let period = (trigger + 1..on.unwrap_or(0))
                    .find(|&i| matches!(word(i), "BEFORE" | "AFTER" | "INSTEAD"));
                if let (Some(on), Some(period)) = (on, period) {
                    if word(period) != "AFTER" {
                        let events = period + 1..on;
                        let insert = events.clone().any(|i| word(i) == "INSERT");
                        let update = events.clone().any(|i| word(i) == "UPDATE");
                        let end = (on + 1..sig_tokens.len())
                            .find(|&i| {
                                matches!(
                                    word(i),
                                    "FOR"
                                        | "REFERENCING"
                                        | "WHEN"
                                        | "EXECUTE"
                                        | "FROM"
                                        | "NOT"
                                        | "DEFERRABLE"
                                        | "INITIALLY"
                                        | "FOLLOWS"
                                        | "PRECEDES"
                                )
                            })
                            .unwrap_or(sig_tokens.len());
                        if let Some(name) = split_object_names(&sig_tokens[on + 1..end]).first() {
                            let name = self.catalog.qualified_name(name);
                            self.note_before_trigger(&name, insert, update);
                        }
                    }
                }
                return;
            }
        }

        // Which definition statement is this, and where does its name start?
        let (kind, name_start) = if word(0) == "CREATE" {
            // Find the object keyword, skipping modifiers such as OR REPLACE, TEMP,
            // UNLOGGED, MATERIALIZED or MySQL's ALGORITHM = x / DEFINER = x
            let object = (1..words.len().min(16)).find_map(|i| {
                let kind = match word(i) {
                    "TABLE" => Some("CREATE TABLE"),
                    "VIEW" => Some("CREATE VIEW"),
                    "TYPE" => Some("CREATE TYPE"),
                    "FUNCTION" | "PROCEDURE" | "TRIGGER" | "INDEX" | "EVENT" | "AGGREGATE"
                    | "OPERATOR" | "DOMAIN" | "SEQUENCE" | "SCHEMA" | "EXTENSION" | "RULE"
                    | "POLICY" | "ROLE" | "USER" | "DATABASE" | "SERVER" | "LANGUAGE" | "CAST"
                    | "COLLATION" | "PUBLICATION" | "SUBSCRIPTION" | "STATISTICS"
                    | "CONVERSION" | "TEXT" | "FOREIGN" | "TABLESPACE" | "ACCESS" => None,
                    _ => return None,
                };
                Some((kind, i + 1))
            });
            let Some((Some(kind), name_start)) = object else {
                // An unparsed `CREATE UNIQUE INDEX`: unique keys aren't all known
                if words.iter().any(|w| w == "UNIQUE") {
                    self.catalog.unknown_unique_keys = true;
                }
                return;
            };
            (kind, name_start)
        } else if word(0) == "ALTER" && word(1) == "TABLE" {
            if words
                .iter()
                .any(|w| matches!(w.as_str(), "UNIQUE" | "PRIMARY" | "EXCLUDE"))
            {
                self.catalog.unknown_unique_keys = true;
            }
            if words
                .iter()
                .any(|w| matches!(w.as_str(), "FOREIGN" | "REFERENCES" | "CONSTRAINT"))
            {
                self.catalog.unknown_foreign_keys = true;
            }
            ("ALTER TABLE", 2)
        } else {
            return;
        };

        // Retry `CREATE TABLE c (LIKE p INCLUDING ...)` / `... WITH NO DATA` without
        // those clauses
        if matches!(kind, "CREATE TABLE" | "CREATE VIEW" | "ALTER TABLE")
            && self.retry_without_unsupported_clauses(stmt)
        {
            return;
        }

        // Skip IF [NOT] EXISTS / ONLY before the object name
        let mut i = name_start;
        let mut if_not_exists = false;
        loop {
            match word(i) {
                "IF" if word(i + 1) == "NOT" => {
                    if_not_exists = true;
                    i += 3;
                }
                "IF" => i += 2,
                "ONLY" => i += 1,
                _ => break,
            }
        }
        let name_tokens = sig_tokens.get(i..).unwrap_or(&[]);
        let name_len = object_name_len(name_tokens);

        if kind == "CREATE TABLE"
            && word(i + name_len) == "PARTITION"
            && word(i + name_len + 1) == "OF"
            && self.process_partition_of(name_tokens, if_not_exists)
        {
            return;
        }
        let name = split_object_names(&name_tokens[..name_len])
            .into_iter()
            .next()
            .map(|n| n.to_string());

        if kind == "ALTER TABLE" {
            // Only warn for operations that change columns; constraints, OWNER TO,
            // ENABLE TRIGGER, REPLICA IDENTITY, ATTACH PARTITION, ... don't affect
            // name resolution
            let op = i + name_len;
            let changes_columns = match word(op) {
                "ADD" | "DROP" => !matches!(
                    word(op + 1),
                    "CONSTRAINT"
                        | "PRIMARY"
                        | "FOREIGN"
                        | "UNIQUE"
                        | "CHECK"
                        | "EXCLUDE"
                        | "INDEX"
                        | "KEY"
                        | "FULLTEXT"
                        | "SPATIAL"
                        | "PARTITION"
                ),
                "ALTER" | "RENAME" | "MODIFY" | "CHANGE" => {
                    !matches!(word(op + 1), "CONSTRAINT" | "CHECK" | "INDEX" | "KEY")
                }
                _ => false,
            };
            if !changes_columns {
                return;
            }
        }

        // Location of the statement in the original input
        let (base_line, base_column) = line_column_at(sql, offset);
        let start = first.span.start;
        let (line, column) = absolute_location(
            (base_line, base_column),
            start.line as usize,
            start.column as usize,
        );
        let start_offset =
            offset + byte_offset_of(stmt, start.line as usize, start.column as usize).unwrap_or(0);
        let length = sql[start_offset..]
            .lines()
            .next()
            .map_or(0, |l| l.trim_end().len())
            .max(1);

        self.catalog.skipped_definitions.push(SkippedDefinition {
            kind: kind.to_string(),
            name: name.clone(),
            line: None,
        });

        let parser_message = relocate_parser_message(&err.to_string(), (base_line, base_column));
        let what = match &name {
            Some(name) => format!("{kind} statement for '{name}'"),
            None => format!("{kind} statement"),
        };
        self.diagnostics.push(
            Diagnostic::warning(
                DiagnosticKind::ParseError,
                format!("Skipped {what} that could not be parsed: {parser_message}"),
            )
            .with_span(Span {
                offset: start_offset,
                length,
                line,
                column,
            })
            .with_help(
                "The statement was ignored, so queries that use it may report missing tables or columns",
            ),
        );
    }

    /// Re-parse a CREATE TABLE / CREATE VIEW statement without the clauses
    /// [`mask_unsupported_clauses`] removes, applying `INHERITS` itself. Returns true
    /// if something was removed and the statement then parsed and was applied.
    fn retry_without_unsupported_clauses(&mut self, stmt: &str) -> bool {
        let Some((masked, parents, include_constraints)) =
            mask_unsupported_clauses(self.dialect, stmt, true)
        else {
            return false;
        };
        let dialect = self.dialect.parser_dialect();
        let Ok(stmts) = crate::stack::parse_sql(dialect.as_ref(), &masked) else {
            return false;
        };
        for stmt in stmts {
            self.process_statement(&stmt);
            if let Statement::CreateTable(create) = &stmt {
                if !parents.is_empty() {
                    let child = self.catalog.qualified_name(&create.name);
                    self.inherit_columns(&child, &parents);
                }
                if include_constraints {
                    let child = self.catalog.qualified_name(&create.name);
                    let like_source = match &create.like {
                        Some(
                            CreateTableLikeKind::Parenthesized(like)
                            | CreateTableLikeKind::Plain(like),
                        ) => Some(&like.name),
                        _ => create.columns.iter().find_map(like_pseudo_column),
                    };
                    if let Some(source_name) = like_source {
                        let source = self.catalog.qualified_name(source_name);
                        if let Some(source_table) = self.catalog.get_table(&source) {
                            let check_constraints = source_table.check_constraints.clone();
                            let dialect = self.dialect;
                            if let Some(child_table) = self.catalog.get_table_mut(&child) {
                                copy_check_constraints(child_table, check_constraints, dialect);
                            }
                        }
                    }
                }
            }
        }
        true
    }

    /// Put the columns of `parents` (in order) before the columns of `child`,
    /// as PostgreSQL table inheritance does
    fn inherit_columns(&mut self, child: &QualifiedName, parents: &[ObjectName]) {
        let mut inherited: indexmap::IndexMap<String, ColumnDef> = indexmap::IndexMap::new();
        for parent in parents {
            let parent_name = self.catalog.qualified_name(parent);
            match self.catalog.get_table(&parent_name) {
                Some(parent_table) => {
                    for (name, col) in &parent_table.columns {
                        let mut col = col.clone();
                        col.is_primary_key = false;
                        inherited.entry(name.clone()).or_insert(col);
                    }
                }
                None => self.diagnostics.push(Diagnostic::warning(
                    DiagnosticKind::TableNotFound,
                    format!(
                        "Table '{child}' inherits from table '{parent_name}' which was not found in schema"
                    ),
                )),
            }
        }
        if let Some(table) = self.catalog.get_table_mut(child) {
            for (name, col) in std::mem::take(&mut table.columns) {
                // A column redeclared in the child is merged with the inherited one
                match inherited.keys().position(|k| k.eq_ignore_ascii_case(&name)) {
                    Some(index) => {
                        inherited.shift_remove_index(index);
                        inherited.shift_insert(index, name, col);
                    }
                    None => {
                        inherited.insert(name, col);
                    }
                }
            }
            table.columns = inherited;
        }
    }

    /// Apply `CREATE TABLE name PARTITION OF parent ...`: the partition has its
    /// parent's columns. `tokens` start at the table name. Returns true if applied.
    fn process_partition_of(&mut self, tokens: &[&Token], if_not_exists: bool) -> bool {
        let name_len = object_name_len(tokens);
        let (Some(name), Some(parent)) = (
            split_object_names(&tokens[..name_len]).into_iter().next(),
            split_object_names(tokens.get(name_len + 2..).unwrap_or(&[]))
                .into_iter()
                .next(),
        ) else {
            return false;
        };
        let name = self.catalog.qualified_name(&name);
        if if_not_exists && self.relation_exists(&name) {
            return true;
        }
        let parent = self.catalog.qualified_name(&parent);
        let Some(parent_table) = self.catalog.get_table(&parent) else {
            return false;
        };
        let mut table = TableDef::new(name);
        table.columns.clone_from(&parent_table.columns);
        table.primary_key.clone_from(&parent_table.primary_key);
        // A unique index of the parent is also an index of the partition
        table.unknown_unique_keys = true;
        table
            .check_constraints
            .clone_from(&parent_table.check_constraints);
        self.catalog.add_table(table);
        true
    }

    /// Apply `ALTER TYPE name ADD VALUE ...` / `RENAME VALUE ...` / `RENAME TO ...`
    /// to an enum
    fn process_alter_type(&mut self, alter: &AlterType) {
        let enum_name = self.catalog.qualified_name(&alter.name).to_string();
        if let AlterTypeOperation::Rename(rename) = &alter.operation {
            let new_name = self.catalog.ident_name(&rename.new_name);
            if let Some(mut def) = self.catalog.get_enum(&enum_name).cloned() {
                self.catalog.drop_enum(&enum_name);
                def.name = new_name;
                self.catalog.add_enum(def);
            }
            return;
        }
        let Some(def) = self.catalog.get_enum_mut(&enum_name) else {
            return;
        };
        match &alter.operation {
            AlterTypeOperation::AddValue(add) => {
                let value = add.value.value.clone();
                if def.values.contains(&value) {
                    return;
                }
                let (anchor, insert_after) = match &add.position {
                    Some(AlterTypeAddValuePosition::Before(a)) => (Some(a), false),
                    Some(AlterTypeAddValuePosition::After(a)) => (Some(a), true),
                    None => (None, false),
                };
                match anchor.and_then(|a| def.values.iter().position(|v| *v == a.value)) {
                    Some(pos) if insert_after => def.values.insert(pos + 1, value),
                    Some(pos) => def.values.insert(pos, value),
                    None => def.values.push(value),
                }
            }
            AlterTypeOperation::RenameValue(rename) => {
                if let Some(v) = def.values.iter_mut().find(|v| **v == rename.from.value) {
                    v.clone_from(&rename.to.value);
                }
            }
            AlterTypeOperation::Rename(_) => {}
        }
    }

    /// Apply `ALTER TYPE name ADD VALUE ...` / `RENAME VALUE ...` / `RENAME TO ...`
    /// (in a statement sqlparser doesn't parse) to an enum. `tokens` start after
    /// `ALTER TYPE`.
    fn process_alter_type_tokens(&mut self, tokens: &[&Token]) {
        let name_len = object_name_len(tokens);
        let Some(name) = split_object_names(&tokens[..name_len]).into_iter().next() else {
            return;
        };
        let enum_name = self.catalog.qualified_name(&name).to_string();
        let rest = &tokens[name_len..];
        let kw = |i: usize| match rest.get(i) {
            Some(Token::Word(w)) if w.quote_style.is_none() => w.value.to_uppercase(),
            _ => String::new(),
        };
        let string = |i: usize| match rest.get(i) {
            Some(Token::SingleQuotedString(s)) => Some(s.replace("''", "'")),
            Some(Token::EscapedStringLiteral(s)) => Some(s.clone()),
            _ => None,
        };

        // Name of the type in a RENAME TO, computed before borrowing the enum
        let renamed_to = if kw(0) == "RENAME" && kw(1) == "TO" {
            split_object_names(&rest[2..])
                .into_iter()
                .next()
                .map(|n| self.catalog.qualified_name(&n).name)
        } else {
            None
        };

        if let Some(new_name) = renamed_to {
            if let Some(mut def) = self.catalog.get_enum(&enum_name).cloned() {
                self.catalog.drop_enum(&enum_name);
                def.name = new_name;
                self.catalog.add_enum(def);
            }
            return;
        }

        let Some(def) = self.catalog.get_enum_mut(&enum_name) else {
            return;
        };
        match (kw(0).as_str(), kw(1).as_str()) {
            ("ADD", "VALUE") => {
                let mut i = 2;
                if kw(2) == "IF" && kw(3) == "NOT" && kw(4) == "EXISTS" {
                    i = 5;
                }
                let Some(value) = string(i) else {
                    return;
                };
                if def.values.contains(&value) {
                    return;
                }
                let anchor = string(i + 2).and_then(|a| def.values.iter().position(|v| *v == a));
                match (kw(i + 1).as_str(), anchor) {
                    ("BEFORE", Some(pos)) => def.values.insert(pos, value),
                    ("AFTER", Some(pos)) => def.values.insert(pos + 1, value),
                    _ => def.values.push(value),
                }
            }
            ("RENAME", "VALUE") => {
                if let (Some(old), true, Some(new)) = (string(2), kw(3) == "TO", string(4)) {
                    if let Some(v) = def.values.iter_mut().find(|v| **v == old) {
                        *v = new;
                    }
                }
            }
            _ => {}
        }
    }

    /// Process a single SQL statement
    fn process_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::CreateTable(create) => {
                self.process_create_table(create);
            }
            Statement::CreateType {
                name,
                representation: Some(representation),
            } => {
                self.process_create_type(name, representation);
            }
            Statement::CreateView(view) => {
                self.catalog.note_database(&view.name);
                let qualified = self.catalog.qualified_name(&view.name);
                if view.if_not_exists && self.relation_exists(&qualified) {
                    return;
                }
                self.process_create_view(qualified, &view.columns, &view.query, view.materialized);
            }
            Statement::AlterTable(alter) => {
                self.process_alter_table(&alter.name, &alter.operations);
            }
            Statement::AlterType(alter) => self.process_alter_type(alter),
            Statement::CreateTrigger(trigger)
                if matches!(
                    trigger.period,
                    Some(TriggerPeriod::Before | TriggerPeriod::InsteadOf)
                ) =>
            {
                let events = &trigger.events;
                let insert = events.iter().any(|e| matches!(e, TriggerEvent::Insert));
                let update = events.iter().any(|e| matches!(e, TriggerEvent::Update(_)));
                let name = self.catalog.qualified_name(&trigger.table_name);
                self.note_before_trigger(&name, insert, update);
            }
            // `SELECT ... INTO [TEMP] t` creates `t`
            Statement::Query(query) => {
                if let Some(into) = select_into(query) {
                    self.process_select_into(&into, query);
                }
            }
            Statement::Set(set) => {
                if let Some(values) = search_path_values(set) {
                    self.process_search_path(values);
                }
            }
            // MySQL `USE db` / `USE SCHEMA s`: unqualified names are in that
            // database for the rest of the file
            Statement::Use(use_stmt) => {
                if let Some(schema) = used_schema(use_stmt) {
                    self.catalog.search_path = vec![self.catalog.ident_name(schema)];
                }
            }
            Statement::CreateIndex(index) if index.unique => self.process_unique_index(index),
            Statement::Drop {
                object_type, names, ..
            } => {
                for name in names {
                    match object_type {
                        ObjectType::Table => self.process_drop_table(name),
                        ObjectType::MaterializedView => {
                            let name = self.catalog.qualified_name(name);
                            self.catalog.drop_view(&name);
                        }
                        ObjectType::View => {
                            let location = self.name_location(name);
                            let name = self.catalog.qualified_name(name);
                            if self.catalog.view_exists(&name) {
                                self.catalog.drop_view(&name);
                                self.catalog.record_dropped_relation(name, location);
                            }
                        }
                        ObjectType::Type => {
                            let name = self.catalog.qualified_name(name);
                            self.catalog.drop_enum(&name.to_string());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Record `CREATE UNIQUE INDEX` as a unique key of its table. A partial or
    /// expression index makes the table's unique keys unknown.
    fn process_unique_index(&mut self, index: &sqlparser::ast::CreateIndex) {
        let name = self.catalog.qualified_name(&index.table_name);
        let columns: Option<Vec<String>> = index
            .columns
            .iter()
            .map(|c| match &c.column.expr {
                Expr::Identifier(ident) => Some(ident.value.clone()),
                _ => None,
            })
            .collect();
        let index_name = index.name.as_ref().map(ToString::to_string);
        let Some(table) = self.catalog.get_table_mut(&name) else {
            return;
        };
        match columns {
            Some(columns) if index.predicate.is_none() => {
                table.unique_constraints.push(UniqueConstraintDef {
                    name: index_name,
                    columns,
                    deferrable: false,
                });
            }
            _ => table.unknown_unique_keys = true,
        }
    }

    /// Whether a table or view with this name exists
    /// Record a BEFORE / INSTEAD OF row trigger on `table` for INSERT and / or UPDATE
    fn note_before_trigger(&mut self, table: &QualifiedName, insert: bool, update: bool) {
        if let Some(table) = self.catalog.get_table_mut(table) {
            table.before_insert_trigger |= insert;
            table.before_update_trigger |= update;
        }
    }

    fn relation_exists(&self, name: &QualifiedName) -> bool {
        self.catalog.relation_exists_exactly(name)
    }

    /// Where the node at `span` (of the statement being applied) is, for hints
    /// on what it renamed or dropped: `None` when neither the schema file nor
    /// the line is known
    fn location(&self, span: sqlparser::tokenizer::Span) -> Option<SourceLocation> {
        if !self.query_file && self.source_file.is_none() {
            return None;
        }
        let line = match usize::try_from(span.start.line) {
            Ok(line) if line > 0 => line + self.line_offset,
            _ => self.statement_line?,
        };
        Some(SourceLocation {
            file: self.source_file.clone(),
            line,
        })
    }

    /// [`Self::location`] of an object name
    fn name_location(&self, name: &ObjectName) -> Option<SourceLocation> {
        self.location(
            name.0
                .first()
                .map_or(sqlparser::tokenizer::Span::empty(), |ident| {
                    ident.ident().span
                }),
        )
    }

    /// Span in the input of an object name, for schema warnings (with a line and
    /// column but no byte offset)
    fn name_span(&self, name: &ObjectName) -> Option<Span> {
        let start = name.0.first()?.ident().span.start;
        let end = name.0.last()?.ident().span.end;
        if start.line == 0 {
            return None;
        }
        let (line, column) = absolute_location(
            (self.line_offset + 1, self.column_offset),
            usize::try_from(start.line).ok()?,
            usize::try_from(start.column).ok()?,
        );
        let length = if end.line == start.line && end.column > start.column {
            usize::try_from(end.column - start.column).unwrap_or(1)
        } else {
            1
        };
        Some(Span::with_location(line, column, length))
    }

    /// Process CREATE TABLE statement
    fn process_create_table(&mut self, create: &sqlparser::ast::CreateTable) {
        self.catalog.note_database(&create.name);
        let name = self.catalog.qualified_name(&create.name);
        self.catalog.restore_relation(&name);
        if create.if_not_exists && self.relation_exists(&name) {
            return;
        }
        let mut table = TableDef::new(name.clone());
        table.strict = create.strict
            || match &create.table_options {
                sqlparser::ast::CreateTableOptions::Plain(opts)
                | sqlparser::ast::CreateTableOptions::With(opts)
                | sqlparser::ast::CreateTableOptions::Options(opts) => {
                    opts.iter().any(|opt| match opt {
                        sqlparser::ast::SqlOption::Ident(ident) => {
                            ident.value.eq_ignore_ascii_case("STRICT")
                        }
                        _ => false,
                    })
                }
                _ => false,
            };

        // MySQL `CREATE TABLE t2 LIKE t`: copy the definition
        let (like, is_mysql_like) = match &create.like {
            Some(CreateTableLikeKind::Plain(like)) => (Some(&like.name), true),
            Some(CreateTableLikeKind::Parenthesized(like)) => (Some(&like.name), false),
            None => (create.clone.as_ref(), false),
        };
        if let Some(like) = like {
            let source = self.catalog.qualified_name(like);
            match self.catalog.get_table(&source) {
                Some(source_table) => {
                    table.columns.clone_from(&source_table.columns);
                    if is_mysql_like {
                        table.primary_key.clone_from(&source_table.primary_key);
                        table
                            .unique_constraints
                            .clone_from(&source_table.unique_constraints);
                        copy_check_constraints(
                            &mut table,
                            source_table.check_constraints.clone(),
                            self.dialect,
                        );
                    }
                    table.strict = source_table.strict;
                }
                None => self.warn_like_source_missing(&name, &source),
            }
        }

        // Process columns (Hive-format `PARTITIONED BY (c TYPE)` adds partition columns)
        let partition_columns = match &create.hive_distribution {
            sqlparser::ast::HiveDistributionStyle::PARTITIONED { columns } => columns.as_slice(),
            _ => &[],
        };
        for column in create.columns.iter().chain(partition_columns) {
            // `CREATE TABLE c (LIKE p)` parses as a column named LIKE of type `p`
            if let Some(source) = like_pseudo_column(column) {
                self.copy_like_columns(&mut table, source);
                continue;
            }

            let (mut col_def, constraints) = build_column(
                &self.catalog,
                self.dialect,
                &name.name,
                &column.name.value,
                &column.data_type,
                &column.options,
            );
            col_def.case_sensitive = self.catalog.is_case_sensitive_column(&column.name);
            merge_constraints(&mut table, constraints, self.dialect);
            table.columns.insert(col_def.name.clone(), col_def);
        }

        // CREATE TABLE ... AS SELECT: infer column names from the query
        if let Some(query) = &create.query {
            if create.columns.is_empty()
                && !self.add_query_columns(&mut table, query, "CREATE TABLE ... AS")
            {
                return;
            }
        }

        // Process table constraints
        for constraint in &create.constraints {
            self.process_table_constraint(&mut table, constraint);
        }

        // `CREATE TABLE p1 PARTITION OF p ...`: the parent's columns
        if let Some(parent) = &create.partition_of {
            self.add_partition(table, parent);
            return;
        }

        self.catalog.add_table(table);
        // PostgreSQL `INHERITS (parent [, ...])`
        if let Some(parents) = &create.inherits {
            self.inherit_columns(&name, parents);
        }
    }

    /// Add a partition (`CREATE TABLE p1 PARTITION OF p ...`) with the columns,
    /// primary key and CHECK constraints of its parent. A partition of a table the
    /// schema doesn't define is skipped with a warning.
    fn add_partition(&mut self, mut table: TableDef, parent: &ObjectName) {
        let parent_name = self.catalog.qualified_name(parent);
        let Some(parent_table) = self.catalog.get_table(&parent_name) else {
            self.diagnostics.push(
                Diagnostic::warning(
                    DiagnosticKind::ParseError,
                    format!(
                        "Skipped CREATE TABLE statement for '{}': it is a partition of table '{parent_name}' which was not found in schema",
                        table.name
                    ),
                )
                .with_help("Ensure the parent table is created before its partitions"),
            );
            self.catalog.skipped_definitions.push(SkippedDefinition {
                kind: "CREATE TABLE".to_string(),
                name: Some(table.name.to_string()),
                line: self.statement_line,
            });
            return;
        };
        table.columns.clone_from(&parent_table.columns);
        table.primary_key.clone_from(&parent_table.primary_key);
        // A unique index of the parent is also an index of the partition
        table.unknown_unique_keys = true;
        table
            .check_constraints
            .clone_from(&parent_table.check_constraints);
        self.catalog.add_table(table);
    }

    /// Add the output columns of `query` to the table it creates (`statement`, for
    /// the warning when they can't be inferred). Returns false if, in a query file,
    /// the table was instead defined as a relation with unknown columns.
    fn add_query_columns(&mut self, table: &mut TableDef, query: &Query, statement: &str) -> bool {
        let name = &table.name;
        match analyzer::query_output_columns(&self.catalog, self.dialect, query) {
            Some(columns) => {
                for (col_name, data_type) in columns {
                    table
                        .columns
                        .entry(col_name.clone())
                        .or_insert_with(|| ColumnDef::new(col_name, data_type));
                }
            }
            None if self.query_file => {
                // A view without columns is one whose columns are unknown
                self.catalog.drop_table(name);
                self.catalog.add_view(ViewDef {
                    name: name.clone(),
                    columns: Vec::new(),
                    column_types: Vec::new(),
                    materialized: false,
                });
                return false;
            }
            None => self.diagnostics.push(
                Diagnostic::warning(
                    DiagnosticKind::ParseError,
                    format!(
                        "Could not determine the columns of table '{name}' created by {statement}"
                    ),
                )
                .with_help("Queries that reference its columns may report missing columns"),
            ),
        }
        true
    }

    /// `SELECT ... INTO [TEMP | UNLOGGED] [TABLE] t`: create `t` with the query's
    /// output columns
    fn process_select_into(&mut self, into: &ObjectName, query: &Query) {
        let name = self.catalog.qualified_name(into);
        let mut table = TableDef::new(name);
        if self.add_query_columns(&mut table, query, "SELECT ... INTO") {
            self.catalog.add_table(table);
        }
    }

    /// `SET [LOCAL] search_path TO a, b` / `= 'a, b'` / `TO DEFAULT`: unqualified
    /// names are looked up in these schemas, in order
    fn process_search_path(&mut self, values: &[Expr]) {
        let mut path = Vec::new();
        for value in values {
            match value {
                Expr::Identifier(ident) => path.push(self.catalog.ident_name(ident)),
                Expr::Value(ValueWithSpan {
                    value: Value::SingleQuotedString(list),
                    ..
                }) => {
                    for part in list.split(',').map(str::trim) {
                        match part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                            Some(quoted) => path.push(quoted.to_string()),
                            None if self.catalog.case_sensitive_names => {
                                path.push(part.to_lowercase());
                            }
                            None => path.push(part.to_string()),
                        }
                    }
                }
                _ => {}
            }
        }
        // `$user` (a schema named after the role) and the system schemas hold no
        // tables of the schema input; `DEFAULT` restores the default
        path.retain(|s| {
            !s.is_empty()
                && s != "$user"
                && !s.eq_ignore_ascii_case("pg_catalog")
                && !s.eq_ignore_ascii_case("pg_temp")
                && !s.eq_ignore_ascii_case("default")
        });
        self.catalog.search_path = path;
    }

    /// Copy the columns of `source` into `table` (`CREATE TABLE c (LIKE p)`)
    fn copy_like_columns(&mut self, table: &mut TableDef, source: &ObjectName) {
        let source = self.catalog.qualified_name(source);
        if let Some(source_table) = self.catalog.get_table(&source) {
            for (col_name, col) in &source_table.columns {
                let mut col = col.clone();
                // Constraints (and with them primary keys) are only copied with
                // INCLUDING options; NOT NULL always is
                col.is_primary_key = false;
                table.columns.insert(col_name.clone(), col);
            }
        } else if let Some(view) = self.catalog.get_view(&source) {
            for col_name in &view.columns {
                table
                    .columns
                    .insert(col_name.clone(), ColumnDef::new(col_name, SqlType::Unknown));
            }
        } else {
            self.warn_like_source_missing(&table.name, &source);
        }
    }

    fn warn_like_source_missing(&mut self, table: &QualifiedName, source: &QualifiedName) {
        self.diagnostics.push(
            Diagnostic::warning(
                DiagnosticKind::TableNotFound,
                format!(
                    "CREATE TABLE '{table}' copies table '{source}' (LIKE) which was not found in schema"
                ),
            )
            .with_help("Ensure the referenced table is created before the LIKE"),
        );
    }

    /// Process CREATE VIEW statement
    fn process_create_view(
        &mut self,
        qualified: QualifiedName,
        columns: &[sqlparser::ast::ViewColumnDef],
        query: &Query,
        materialized: bool,
    ) {
        // Column names: explicit column list or inferred from SELECT, with the types
        // inferred from SELECT. An empty list means the columns could not be determined.
        let inferred =
            analyzer::query_output_columns(&self.catalog, self.dialect, query).unwrap_or_default();
        let (column_names, column_types) = if columns.is_empty() {
            inferred.into_iter().unzip()
        } else {
            columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let data_type = inferred.get(i).map_or(SqlType::Unknown, |(_, t)| t.clone());
                    (c.name.value.clone(), data_type)
                })
                .unzip()
        };

        self.catalog.restore_relation(&qualified);
        let view = ViewDef {
            name: qualified,
            columns: column_names,
            column_types,
            materialized,
        };
        self.catalog.add_view(view);
    }

    /// Process ALTER TABLE statement
    fn process_alter_table(&mut self, name: &ObjectName, operations: &[AlterTableOperation]) {
        // Skip ALTER TABLE if it contains no schema-affecting operations.
        // Operations like OWNER TO, ENABLE/DISABLE TRIGGER, etc. don't affect
        // the schema catalog and should not produce warnings.
        let has_schema_operations = operations.iter().any(|op| {
            matches!(
                op,
                AlterTableOperation::AddColumn { .. }
                    | AlterTableOperation::DropColumn { .. }
                    | AlterTableOperation::RenameColumn { .. }
                    | AlterTableOperation::RenameTable { .. }
                    | AlterTableOperation::AddConstraint { .. }
                    | AlterTableOperation::DropConstraint { .. }
                    | AlterTableOperation::RenameConstraint { .. }
                    | AlterTableOperation::AlterColumn { .. }
                    | AlterTableOperation::ChangeColumn { .. }
                    | AlterTableOperation::ModifyColumn { .. }
            )
        });

        if !has_schema_operations {
            return;
        }

        let table_name = self.catalog.qualified_name(name);

        // Check if table exists
        if !self.catalog.table_exists(&table_name) {
            let mut warning = Diagnostic::warning(
                DiagnosticKind::TableNotFound,
                format!(
                    "ALTER TABLE references table '{table_name}' which was not found in schema"
                ),
            )
            .with_help("Ensure the CREATE TABLE statement appears before ALTER TABLE");
            warning.span = self.name_span(name);
            self.diagnostics.push(warning);
            return;
        }

        for operation in operations {
            match operation {
                AlterTableOperation::AddColumn { column_def, .. } => {
                    let (mut col, constraints) = build_column(
                        &self.catalog,
                        self.dialect,
                        &table_name.name,
                        &column_def.name.value,
                        &column_def.data_type,
                        &column_def.options,
                    );
                    col.case_sensitive = self.catalog.is_case_sensitive_column(&column_def.name);
                    let dialect = self.dialect;
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        merge_constraints(table, constraints, dialect);
                        table.forget_former_column(&col.name);
                        table.columns.insert(col.name.clone(), col);
                    }
                }
                AlterTableOperation::DropColumn { column_names, .. } => {
                    let dialect = self.dialect;
                    for column_name in column_names {
                        let location = self.location(column_name.span);
                        if let Some(table) = self.catalog.get_table_mut(&table_name) {
                            if let Some(index) = column_index(table, &column_name.value) {
                                if let Some((name, _)) = table.columns.shift_remove_index(index) {
                                    table
                                        .check_constraints
                                        .retain(|c| !check_references_column(c, &name, dialect));
                                    table.record_column_drop(&name, location);
                                }
                            }
                        }
                    }
                }
                AlterTableOperation::RenameColumn {
                    old_column_name,
                    new_column_name,
                } => {
                    let case_sensitive = self.catalog.is_case_sensitive_column(new_column_name);
                    let location = self.location(old_column_name.span);
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        if let Some(index) = column_index(table, &old_column_name.value) {
                            let mut col = table.columns[index].clone();
                            col.name.clone_from(&new_column_name.value);
                            col.case_sensitive = case_sensitive;
                            let old_name = table.columns[index].name.clone();
                            table.record_column_rename(&old_name, &col.name, location);
                            replace_column(table, index, col);
                        }
                    }
                }
                AlterTableOperation::AlterColumn { column_name, op } => {
                    if let Some(col) = self.catalog.get_table_mut(&table_name).and_then(|t| {
                        column_index(t, &column_name.value).map(|i| &mut t.columns[i])
                    }) {
                        apply_alter_column(col, op, self.dialect);
                    }
                }
                AlterTableOperation::ModifyColumn {
                    col_name,
                    data_type,
                    options,
                    ..
                }
                | AlterTableOperation::ChangeColumn {
                    old_name: col_name,
                    data_type,
                    options,
                    ..
                } => {
                    let new_name = match operation {
                        AlterTableOperation::ChangeColumn { new_name, .. } => new_name,
                        _ => col_name,
                    };
                    let options: Vec<ColumnOptionDef> = options
                        .iter()
                        .map(|option| ColumnOptionDef {
                            name: None,
                            option: option.clone(),
                        })
                        .collect();
                    let (col, constraints) = build_column(
                        &self.catalog,
                        self.dialect,
                        &table_name.name,
                        &new_name.value,
                        data_type,
                        &options,
                    );
                    let location = self.location(col_name.span);
                    let dialect = self.dialect;
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        merge_constraints(table, constraints, dialect);
                        if let Some(index) = column_index(table, &col_name.value) {
                            let old_name = table.columns[index].name.clone();
                            table.record_column_rename(&old_name, &col.name, location);
                            replace_column(table, index, col);
                        } else {
                            table.forget_former_column(&col.name);
                            table.columns.insert(col.name.clone(), col);
                        }
                    }
                }
                AlterTableOperation::RenameTable {
                    table_name:
                        RenameTableNameKind::As(new_name) | RenameTableNameKind::To(new_name),
                } => {
                    let location = self.name_location(new_name);
                    let new_qualified = self.catalog.qualified_name(new_name);
                    // The name the table had (as created, not as written here)
                    let old_name = self
                        .catalog
                        .get_table(&table_name)
                        .map_or_else(|| table_name.clone(), |t| t.name.clone());
                    self.catalog
                        .rename_table(&table_name, new_qualified.name.clone());
                    self.catalog
                        .record_renamed_relation(old_name, &new_qualified.name, location);
                }
                AlterTableOperation::AddConstraint { constraint, .. } => {
                    let mut constraints = TableDef::new(table_name.clone());
                    self.process_table_constraint(&mut constraints, constraint);
                    let dialect = self.dialect;
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        if let Some(pk) = &constraints.primary_key {
                            for col_name in &pk.columns {
                                if let Some(index) = column_index(table, col_name) {
                                    let col = &mut table.columns[index];
                                    col.is_primary_key = true;
                                    col.nullable = false;
                                }
                            }
                        }
                        merge_constraints(table, constraints, dialect);
                    }
                }
                AlterTableOperation::RenameConstraint { old_name, new_name } => {
                    let dialect = self.dialect;
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        rename_table_constraint(table, &old_name.value, &new_name.value, dialect);
                    }
                }
                AlterTableOperation::DropConstraint { name, .. } => {
                    let dialect = self.dialect;
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        drop_table_constraint(table, &name.value, dialect);
                    }
                }
                _ => {
                    // Other ALTER TABLE operations - not yet supported
                }
            }
        }
    }

    /// Process DROP TABLE statement
    fn process_drop_table(&mut self, name: &ObjectName) {
        let location = self.name_location(name);
        let table_name = self.catalog.qualified_name(name);
        let mut existed = self.catalog.table_exists(&table_name);
        self.catalog.drop_table(&table_name);
        if self.query_file {
            // A `CREATE TABLE ... AS` with unknown columns is defined as a view
            existed |= self.catalog.view_exists(&table_name);
            self.catalog.drop_view(&table_name);
        }
        if existed {
            self.catalog.record_dropped_relation(table_name, location);
        }
    }

    /// Process CREATE TYPE statement
    fn process_create_type(
        &mut self,
        name: &ObjectName,
        representation: &UserDefinedTypeRepresentation,
    ) {
        let qualified = self.catalog.qualified_name(name);
        if let UserDefinedTypeRepresentation::Enum { labels } = representation {
            // An unqualified type is created in the first schema of a search path
            let schema = qualified.schema.or_else(|| {
                (!self.catalog.search_path.is_empty())
                    .then(|| self.catalog.creation_schema().to_string())
            });
            let enum_def = EnumTypeDef {
                name: qualified.name,
                values: labels.iter().map(|l| l.value.clone()).collect(),
                schema,
            };
            self.catalog.add_enum(enum_def);
        } else {
            // Composite types and others - not yet supported
        }
    }

    /// Process a table constraint (PRIMARY KEY, FOREIGN KEY, UNIQUE)
    fn process_table_constraint(&mut self, table: &mut TableDef, constraint: &TableConstraint) {
        match constraint {
            TableConstraint::PrimaryKey(pk) => {
                let deferrable = is_deferrable(pk.characteristics.as_ref());
                let pk = PrimaryKeyDef {
                    name: pk.name.as_ref().map(|n| n.value.clone()),
                    columns: pk.columns.iter().map(index_column_name).collect(),
                    deferrable,
                };
                // Mark columns as primary key
                for col_name in &pk.columns {
                    if let Some(index) = column_index(table, col_name) {
                        let col = &mut table.columns[index];
                        col.is_primary_key = true;
                        col.nullable = false;
                    }
                }
                table.primary_key = Some(pk);
            }
            TableConstraint::ForeignKey(fk) => {
                let fk = ForeignKeyDef {
                    name: fk.name.as_ref().map(|n| n.value.clone()),
                    columns: fk.columns.iter().map(|c| c.value.clone()).collect(),
                    references_table: self.catalog.qualified_name(&fk.foreign_table),
                    references_columns: fk
                        .referred_columns
                        .iter()
                        .map(|c| c.value.clone())
                        .collect(),
                };
                table.foreign_keys.push(fk);
            }
            TableConstraint::Unique(unique) => {
                let deferrable = is_deferrable(unique.characteristics.as_ref());
                let unique = UniqueConstraintDef {
                    name: unique.name.as_ref().map(|n| n.value.clone()),
                    columns: unique.columns.iter().map(index_column_name).collect(),
                    deferrable,
                };
                table.unique_constraints.push(unique);
            }
            TableConstraint::Check(check) => {
                let name = check.name.as_ref().map(|n| n.value.clone());
                let expression = check.expr.to_string();
                let generated_name = if name.is_none() {
                    generated_check_name(
                        self.dialect,
                        &table.name.name,
                        None,
                        &expression,
                        &table.check_constraints,
                    )
                } else {
                    None
                };
                let check = CheckConstraintDef {
                    name,
                    expression,
                    enforced: check.enforced.unwrap_or(true),
                    column: None,
                    generated_name,
                };
                table.check_constraints.push(check);
            }
            // `PRIMARY KEY USING INDEX i` / `UNIQUE USING INDEX i` (the key is the
            // index's columns) and exclusion constraints: unique keys aren't all known
            TableConstraint::PrimaryKeyUsingIndex(_)
            | TableConstraint::UniqueUsingIndex(_)
            | TableConstraint::Exclude(_) => table.unknown_unique_keys = true,
            _ => {}
        }
    }

    /// Consume the builder and return the catalog
    pub fn build(self) -> (Catalog, Vec<Diagnostic>) {
        (self.catalog, self.diagnostics)
    }

    /// Get a reference to the current catalog
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

impl Default for SchemaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn is_deferrable(characteristics: Option<&sqlparser::ast::ConstraintCharacteristics>) -> bool {
    let Some(c) = characteristics else {
        return false;
    };
    c.deferrable == Some(true) || c.initially == Some(sqlparser::ast::DeferrableInitial::Deferred)
}

/// Build a column definition from its name, type and options. Table-level
/// constraints declared inline (PRIMARY KEY, UNIQUE, REFERENCES, CHECK) are
/// collected into the returned `TableDef`, to be merged into the owning table.
fn build_column(
    catalog: &Catalog,
    dialect: SqlDialect,
    table_name: &str,
    col_name: &str,
    data_type: &DataType,
    options: &[ColumnOptionDef],
) -> (ColumnDef, TableDef) {
    let mut col = ColumnDef::new(col_name, SqlType::from_ast_in(data_type, dialect));
    let mut constraints = TableDef::new(QualifiedName::new(table_name));

    for option in options {
        let constraint_name = option.name.as_ref().map(|n| n.value.clone());
        match &option.option {
            ColumnOption::Null => col.nullable = true,
            ColumnOption::NotNull => col.nullable = false,
            ColumnOption::Default(expr) => col.default = Some(expr_to_default(expr)),
            ColumnOption::Collation(_) => col.collated = true,
            ColumnOption::PrimaryKey(pk) => {
                col.is_primary_key = true;
                col.nullable = false;
                let deferrable = is_deferrable(pk.characteristics.as_ref());
                constraints.primary_key = Some(PrimaryKeyDef {
                    name: constraint_name,
                    columns: vec![col_name.to_string()],
                    deferrable,
                });
            }
            ColumnOption::Unique(unique) => {
                let deferrable = is_deferrable(unique.characteristics.as_ref());
                constraints.unique_constraints.push(UniqueConstraintDef {
                    name: constraint_name,
                    columns: vec![col_name.to_string()],
                    deferrable,
                });
            }
            ColumnOption::ForeignKey(fk) => {
                constraints.foreign_keys.push(ForeignKeyDef {
                    name: constraint_name,
                    columns: vec![col_name.to_string()],
                    references_table: catalog.qualified_name(&fk.foreign_table),
                    references_columns: fk
                        .referred_columns
                        .iter()
                        .map(|c| c.value.clone())
                        .collect(),
                });
            }
            ColumnOption::Check(check) => {
                constraints.check_constraints.push(CheckConstraintDef {
                    name: constraint_name,
                    expression: check.expr.to_string(),
                    enforced: check.enforced.unwrap_or(true),
                    column: Some(col_name.to_string()),
                    generated_name: None,
                });
            }
            ColumnOption::Generated {
                generated_as,
                generation_expr: None,
                ..
            } => {
                // IDENTITY columns (no generation expression = IDENTITY, not computed)
                use sqlparser::ast::GeneratedAs;
                let kind = match generated_as {
                    GeneratedAs::Always => IdentityKind::Always,
                    GeneratedAs::ByDefault => IdentityKind::ByDefault,
                    GeneratedAs::ExpStored => continue,
                };
                col.identity = Some(kind);
                col.nullable = false; // IDENTITY columns are implicitly NOT NULL
            }
            // Generated (computed) column: `GENERATED ALWAYS AS (expr) STORED`, MySQL
            // and SQLite `[GENERATED ALWAYS] AS (expr) [VIRTUAL | STORED]`
            ColumnOption::Generated {
                generation_expr: Some(_),
                ..
            } => {
                col.generated = true;
            }
            // MySQL AUTO_INCREMENT / SQLite AUTOINCREMENT
            ColumnOption::DialectSpecific(tokens)
                if tokens.iter().any(|t| {
                    matches!(t, Token::Word(w) if w.value.eq_ignore_ascii_case("AUTO_INCREMENT") || w.value.eq_ignore_ascii_case("AUTOINCREMENT"))
                }) =>
            {
                col.nullable = false; // AUTO_INCREMENT/AUTOINCREMENT implies NOT NULL
                col.auto_increment = true;
            }
            _ => {}
        }
    }

    // serial / bigserial / smallserial: NOT NULL with a sequence default
    if is_serial_type(data_type) {
        col.nullable = false;
        if col.default.is_none() {
            col.default = Some(DefaultValue::NextVal(format!(
                "nextval('{table_name}_{col_name}_seq'::regclass)"
            )));
        }
    }

    (col, constraints)
}

/// The `INTO` target of a `SELECT ... INTO t` query
fn select_into(query: &Query) -> Option<ObjectName> {
    let SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    match select.into.as_ref()?.targets.as_slice() {
        [Expr::Identifier(ident)] => Some(ObjectName::from(vec![ident.clone()])),
        [Expr::CompoundIdentifier(idents)] => Some(ObjectName::from(idents.clone())),
        _ => None,
    }
}

/// The name of an index or key column (`PRIMARY KEY (a, b)`); an expression part
/// is kept as written
fn index_column_name(column: &IndexColumn) -> String {
    match &column.column.expr {
        Expr::Identifier(ident) => ident.value.clone(),
        expr => expr.to_string(),
    }
}

/// The schema (MySQL database) a `USE` statement makes current: the last part of
/// `USE db` / `USE SCHEMA db.schema`
fn used_schema(use_stmt: &Use) -> Option<&Ident> {
    match use_stmt {
        Use::Object(name) | Use::Schema(name) => name_last(name),
        _ => None,
    }
}

/// The values a SET statement gives `search_path`, if it sets it
fn search_path_values(set: &Set) -> Option<&[Expr]> {
    match set {
        Set::SingleAssignment {
            variable, values, ..
        } if name_last(variable).is_some_and(|i| i.value.eq_ignore_ascii_case("search_path")) => {
            Some(values)
        }
        _ => None,
    }
}

/// Whether a data type is one of PostgreSQL's serial pseudo-types
fn is_serial_type(data_type: &DataType) -> bool {
    match data_type {
        DataType::Custom(name, modifiers) if modifiers.is_empty() => {
            matches!(
                name.0
                    .last()
                    .map(|i| i.ident().value.to_lowercase())
                    .as_deref(),
                Some("serial" | "serial2" | "serial4" | "serial8" | "smallserial" | "bigserial")
            )
        }
        _ => false,
    }
}

/// Add the constraints collected in `constraints` to `table`
fn merge_constraints(table: &mut TableDef, constraints: TableDef, dialect: SqlDialect) {
    if constraints.primary_key.is_some() {
        table.primary_key = constraints.primary_key;
    }
    table.foreign_keys.extend(constraints.foreign_keys);
    table
        .unique_constraints
        .extend(constraints.unique_constraints);
    copy_check_constraints(table, constraints.check_constraints, dialect);
    table.unknown_unique_keys |= constraints.unknown_unique_keys;
}

fn copy_check_constraints(
    table: &mut TableDef,
    checks: Vec<CheckConstraintDef>,
    dialect: SqlDialect,
) {
    for mut check in checks {
        if check.name.is_none() {
            check.generated_name = generated_check_name(
                dialect,
                &table.name.name,
                check.column.as_deref(),
                &check.expression,
                &table.check_constraints,
            );
        }
        table.check_constraints.push(check);
    }
}

fn generated_check_name(
    dialect: SqlDialect,
    table_name: &str,
    col_name: Option<&str>,
    expression: &str,
    existing: &[CheckConstraintDef],
) -> Option<String> {
    let is_taken = |candidate: &str| {
        existing.iter().any(|c| {
            c.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(candidate))
                || c.generated_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(candidate))
        })
    };
    if dialect == SqlDialect::MySQL {
        for i in 1.. {
            let candidate = format!("{table_name}_chk_{i}");
            if !is_taken(&candidate) {
                return Some(candidate);
            }
        }
        None
    } else if dialect.is_psql() {
        let inferred_col = if col_name.is_none() {
            Parser::new(dialect.parser_dialect().as_ref())
                .try_with_sql(expression)
                .and_then(|mut p| p.parse_expr())
                .ok()
                .and_then(|expr| {
                    let mut cols = std::collections::HashSet::new();
                    analyzer::ranges::collect_referenced_columns(&expr, &mut cols);
                    (cols.len() == 1).then(|| cols.into_iter().next()).flatten()
                })
        } else {
            None
        };
        let base = match col_name.or(inferred_col.as_deref()) {
            Some(col) => format!("{table_name}_{col}_check"),
            None => format!("{table_name}_check"),
        };
        if !is_taken(&base) {
            return Some(base);
        }
        for i in 1.. {
            let candidate = format!("{base}{i}");
            if !is_taken(&candidate) {
                return Some(candidate);
            }
        }
        None
    } else {
        None
    }
}

fn check_references_column(
    check: &CheckConstraintDef,
    col_name: &str,
    dialect: SqlDialect,
) -> bool {
    if check
        .column
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case(col_name))
    {
        return true;
    }
    if let Ok(expr) = Parser::new(dialect.parser_dialect().as_ref())
        .try_with_sql(&check.expression)
        .and_then(|mut p| p.parse_expr())
    {
        let mut cols = std::collections::HashSet::new();
        analyzer::ranges::collect_referenced_columns(&expr, &mut cols);
        return cols.iter().any(|c| c.eq_ignore_ascii_case(col_name));
    }
    check
        .expression
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w.eq_ignore_ascii_case(col_name))
}

fn check_matches_name(check: &CheckConstraintDef, target: &str) -> bool {
    check
        .name
        .as_deref()
        .is_some_and(|n| n.eq_ignore_ascii_case(target))
        || (check.name.is_none()
            && check
                .generated_name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(target)))
}

fn rename_table_constraint(
    table: &mut TableDef,
    old_name: &str,
    new_name: &str,
    dialect: SqlDialect,
) {
    if let Some(check) = table
        .check_constraints
        .iter_mut()
        .find(|c| check_matches_name(c, old_name))
    {
        check.name = Some(new_name.to_string());
        check.generated_name = None;
        return;
    }
    if dialect.is_psql() && old_name.eq_ignore_ascii_case(&format!("{}_check", table.name.name)) {
        if let Some(check) = table
            .check_constraints
            .iter_mut()
            .find(|c| c.name.is_none() && c.column.is_none())
        {
            check.name = Some(new_name.to_string());
            check.generated_name = None;
            return;
        }
    }
    for u in &mut table.unique_constraints {
        if u.name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(old_name))
        {
            u.name = Some(new_name.to_string());
            return;
        }
    }
    for fk in &mut table.foreign_keys {
        if fk
            .name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(old_name))
        {
            fk.name = Some(new_name.to_string());
            return;
        }
    }
    if let Some(pk) = &mut table.primary_key {
        if pk
            .name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(old_name))
        {
            pk.name = Some(new_name.to_string());
        }
    }
}

fn drop_table_constraint(table: &mut TableDef, target: &str, dialect: SqlDialect) {
    let before = table.check_constraints.len();
    table
        .check_constraints
        .retain(|c| !check_matches_name(c, target));
    if table.check_constraints.len() == before
        && dialect.is_psql()
        && target.eq_ignore_ascii_case(&format!("{}_check", table.name.name))
    {
        if let Some(pos) = table
            .check_constraints
            .iter()
            .position(|c| c.name.is_none() && c.column.is_none())
        {
            table.check_constraints.remove(pos);
        }
    }
    table.unique_constraints.retain(|u| {
        !u.name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(target))
    });
    table.foreign_keys.retain(|fk| {
        !fk.name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(target))
    });
    if table
        .primary_key
        .as_ref()
        .and_then(|pk| pk.name.as_deref())
        .is_some_and(|n| n.eq_ignore_ascii_case(target))
    {
        table.primary_key = None;
    }
}

/// Apply an `ALTER TABLE ... ALTER COLUMN` operation to a column
fn apply_alter_column(col: &mut ColumnDef, op: &AlterColumnOperation, dialect: SqlDialect) {
    match op {
        AlterColumnOperation::SetNotNull => col.nullable = false,
        AlterColumnOperation::DropNotNull => col.nullable = true,
        AlterColumnOperation::SetDefault { value } => col.default = Some(expr_to_default(value)),
        AlterColumnOperation::DropDefault => col.default = None,
        AlterColumnOperation::SetDataType { data_type, .. } => {
            col.data_type = SqlType::from_ast_in(data_type, dialect);
        }
        AlterColumnOperation::AddGenerated { generated_as, .. } => {
            use sqlparser::ast::GeneratedAs;
            col.identity = Some(match generated_as {
                Some(GeneratedAs::Always) => IdentityKind::Always,
                _ => IdentityKind::ByDefault,
            });
            col.nullable = false;
        }
    }
}

/// Index of a column by name: exact match first, then ignoring case
fn column_index(table: &TableDef, name: &str) -> Option<usize> {
    table.columns.get_index_of(name).or_else(|| {
        table
            .columns
            .keys()
            .position(|k| k.eq_ignore_ascii_case(name))
    })
}

/// Replace the column at `index` with `col` (which may have a new name), keeping
/// its position and updating constraints that reference a renamed column
fn replace_column(table: &mut TableDef, index: usize, col: ColumnDef) {
    let Some((old_name, _)) = table.columns.shift_remove_index(index) else {
        return;
    };
    let new_name = col.name.clone();
    table.columns.shift_insert(index, new_name.clone(), col);
    if old_name == new_name {
        return;
    }
    let rename = |columns: &mut Vec<String>| {
        for c in columns.iter_mut() {
            if *c == old_name {
                c.clone_from(&new_name);
            }
        }
    };
    if let Some(pk) = &mut table.primary_key {
        rename(&mut pk.columns);
    }
    for fk in &mut table.foreign_keys {
        rename(&mut fk.columns);
    }
    for unique in &mut table.unique_constraints {
        rename(&mut unique.columns);
    }
}

/// Column name [`mask_unsupported_clauses`] writes over the `LIKE` of a `LIKE p`
/// element (same length), so the element parses as a column of type `p`
const LIKE_COLUMN: &str = "_lk_";

/// If `column` is the pseudo-column for `LIKE p` inside a CREATE TABLE column list
/// (see [`LIKE_COLUMN`]), return the source table name
fn like_pseudo_column(column: &sqlparser::ast::ColumnDef) -> Option<&ObjectName> {
    if column.name.quote_style.is_some()
        || !(column.name.value.eq_ignore_ascii_case("LIKE") || column.name.value == LIKE_COLUMN)
        || !column.options.is_empty()
    {
        return None;
    }
    match &column.data_type {
        DataType::Custom(name, modifiers) if modifiers.is_empty() => Some(name),
        _ => None,
    }
}

/// Blank out, keeping every other character in place, the clauses of a CREATE
/// TABLE / CREATE VIEW statement that sqlparser can't parse and that don't change
/// its columns: `UNLOGGED`, `LIKE` options (`INCLUDING x` / `EXCLUDING x`) and a
/// trailing `WITH [NO] DATA`; with `inherits`, also PostgreSQL's
/// `INHERITS (parent [, ...])`, whose parents are returned.
///
/// Returns `None` if `stmt` isn't a CREATE statement or has none of these clauses.
pub(crate) fn mask_unsupported_clauses(
    dialect: SqlDialect,
    stmt: &str,
    inherits: bool,
) -> Option<(String, Vec<ObjectName>, bool)> {
    let parser_dialect = dialect.parser_dialect();
    let tokens = Tokenizer::new(parser_dialect.as_ref(), stmt)
        .with_unescape(false)
        .tokenize_with_location()
        .ok()?;
    let is_word = |t: &Token, kw: &str| matches!(t, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(kw));
    // Indexes (into `tokens`) of significant tokens, and which ones to drop
    let significant: Vec<usize> = (0..tokens.len())
        .filter(|&i| !matches!(tokens[i].token, Token::Whitespace(_)))
        .collect();
    let word = |n: usize, kw: &str| {
        significant
            .get(n)
            .is_some_and(|&i| is_word(&tokens[i].token, kw))
    };
    // PostgreSQL `ALTER TABLE ... ADD COLUMN c varchar ARRAY`, `... MATCH SIMPLE`,
    // `... NOT VALID`; MySQL `ALTER TABLE ... DROP CHECK k`, `... ALTER CHECK k NOT ENFORCED`
    let alter =
        (dialect.is_psql() || dialect == SqlDialect::MySQL) && word(0, "ALTER") && word(1, "TABLE");
    if !word(0, "CREATE") && !alter {
        return None;
    }
    let mut drop = vec![false; tokens.len()];
    // CREATE UNLOGGED TABLE
    if word(1, "UNLOGGED") && word(2, "TABLE") {
        drop[significant[1]] = true;
    }
    let mut include_constraints = false;
    for (n, &i) in significant.iter().enumerate() {
        let is_inc = is_word(&tokens[i].token, "INCLUDING");
        let is_exc = is_word(&tokens[i].token, "EXCLUDING");
        if is_inc || is_exc {
            if let Some(&next) = significant.get(n + 1) {
                if let Token::Word(w) = &tokens[next].token {
                    drop[i] = true;
                    drop[next] = true;
                    if w.value.eq_ignore_ascii_case("CONSTRAINTS")
                        || w.value.eq_ignore_ascii_case("ALL")
                    {
                        include_constraints = is_inc;
                    }
                }
            }
        }
    }
    // Trailing WITH [NO] DATA
    let tail: Vec<usize> = significant
        .iter()
        .rev()
        .skip_while(|&&i| tokens[i].token == Token::SemiColon)
        .take(3)
        .copied()
        .collect();
    if tail
        .first()
        .is_some_and(|&i| is_word(&tokens[i].token, "DATA"))
    {
        let with_at = if tail
            .get(1)
            .is_some_and(|&i| is_word(&tokens[i].token, "NO"))
        {
            2
        } else {
            1
        };
        if tail
            .get(with_at)
            .is_some_and(|&i| is_word(&tokens[i].token, "WITH"))
        {
            for &i in &tail[..=with_at] {
                drop[i] = true;
            }
        }
    }
    // PostgreSQL `INHERITS (parent [, ...])`
    let mut parents = Vec::new();
    if let Some(n) = significant
        .iter()
        .position(|&i| inherits && is_word(&tokens[i].token, "INHERITS"))
    {
        let close = significant[n..]
            .iter()
            .position(|&i| tokens[i].token == Token::RParen)
            .map(|p| n + p);
        if let Some(close) = close {
            if significant
                .get(n + 1)
                .is_some_and(|&i| tokens[i].token == Token::LParen)
            {
                let inner: Vec<&Token> = significant[n + 2..close]
                    .iter()
                    .map(|&i| &tokens[i].token)
                    .collect();
                parents = split_object_names(&inner);
                for &i in &significant[n..=close] {
                    drop[i] = true;
                }
            }
        }
    }
    // Redshift / Databricks table attributes
    let offset = |t: &TokenWithSpan| {
        byte_offset_of(
            stmt,
            t.span.start.line as usize,
            t.span.start.column as usize,
        )
    };
    let mut edits = if alter {
        Vec::new()
    } else {
        super::warehouse::mask_table_attributes(dialect, &tokens, &significant, &offset, &mut drop)
    };
    edits.extend(mask_table_elements(
        dialect,
        &tokens,
        &significant,
        &offset,
        &mut drop,
    ));
    if !drop.contains(&true) && edits.is_empty() {
        return None;
    }

    // Blank the dropped tokens' characters
    let mut blank = vec![false; stmt.len()];
    for (token, _) in tokens.iter().zip(&drop).filter(|(_, &dropped)| dropped) {
        let (start, end) = (token.span.start, token.span.end);
        let start = byte_offset_of(stmt, start.line as usize, start.column as usize)?;
        let end =
            byte_offset_of(stmt, end.line as usize, end.column as usize).unwrap_or(stmt.len());
        blank[start..end].fill(true);
    }
    // ... and the edited ranges, writing their replacement text over them
    let mut replaced = std::collections::HashMap::new();
    let mut insertions: std::collections::HashMap<usize, &str> = std::collections::HashMap::new();
    for edit in &edits {
        let range = edit.range.start..edit.range.end.min(stmt.len());
        blank[range.clone()].fill(true);
        let slice = stmt.get(range.clone())?;
        let mut chars = edit.text.chars();
        let mut last_idx = range.start;
        for (i, _) in slice.char_indices() {
            last_idx = range.start + i;
            if let Some(c) = chars.next() {
                replaced.insert(last_idx, c);
            }
        }
        let remainder = chars.as_str();
        if !remainder.is_empty() {
            insertions.insert(last_idx, remainder);
        }
    }
    let mut masked = String::with_capacity(stmt.len());
    for (i, c) in stmt.char_indices() {
        match replaced.get(&i) {
            Some(&r) => masked.push(r),
            None if blank[i] && c != '\n' => masked.push(' '),
            None => masked.push(c),
        }
        if let Some(extra) = insertions.get(&i) {
            masked.push_str(extra);
        }
    }
    Some((masked, parents, include_constraints))
}

/// Mark for dropping the parts of a CREATE TABLE's column and constraint list that
/// sqlparser can't parse and that don't change its columns:
///
/// - a `LIKE p` element, which sqlparser only parses as the whole list (written
///   over by [`LIKE_COLUMN`])
/// - PostgreSQL exclusion constraints (`[CONSTRAINT name] EXCLUDE USING gist (...)`),
///   with the comma before them, a foreign key's `MATCH SIMPLE | FULL | PARTIAL`, a
///   constraint's `NOT VALID`, and the SQL-standard array type suffix
///   (`varchar ARRAY`, which becomes `varchar[]`); these also in ALTER TABLE
/// - MySQL key parts with a direction or a prefix length (`INDEX (a DESC)`,
///   `KEY k (name(10))`), the index name of a foreign key
///   (`FOREIGN KEY fk_name (col)`) and the precision of floating-point types
///   (`DOUBLE(8,2)`, `FLOAT(5,2)`, `REAL(8,2)`)
fn mask_table_elements(
    dialect: SqlDialect,
    tokens: &[TokenWithSpan],
    significant: &[usize],
    offset: &dyn Fn(&TokenWithSpan) -> Option<usize>,
    drop: &mut [bool],
) -> Vec<super::warehouse::Edit> {
    let mut edits = Vec::new();
    let mysql = dialect == SqlDialect::MySQL;
    if !(mysql || dialect.is_psql()) {
        return edits;
    }
    let token = |n: usize| significant.get(n).map(|&i| &tokens[i].token);
    let word = |n: usize| match token(n) {
        Some(Token::Word(w)) if w.quote_style.is_none() => w.value.to_ascii_uppercase(),
        _ => String::new(),
    };
    let is = |n: usize, t: &Token| token(n) == Some(t);
    if word(0) == "ALTER" && word(1) == "TABLE" {
        for n in 2..significant.len() {
            if mysql && word(n) == "DROP" && word(n + 1) == "CHECK" {
                if let Some(start) = offset(&tokens[significant[n + 1]]) {
                    edits.push(super::warehouse::Edit {
                        range: start..start + "CHECK".len(),
                        text: "CONSTRAINT",
                    });
                }
            } else if word(n) == "ALTER"
                && matches!(word(n + 1).as_str(), "CHECK" | "CONSTRAINT")
                && matches!(token(n + 2), Some(Token::Word(_)))
                && word(n + 3) == "NOT"
                && word(n + 4) == "ENFORCED"
            {
                let kw_len = word(n + 1).len();
                if let (Some(alter_start), Some(kind_start)) = (
                    offset(&tokens[significant[n]]),
                    offset(&tokens[significant[n + 1]]),
                ) {
                    edits.push(super::warehouse::Edit {
                        range: alter_start..alter_start + "ALTER".len(),
                        text: "DROP",
                    });
                    edits.push(super::warehouse::Edit {
                        range: kind_start..kind_start + kw_len,
                        text: "CONSTRAINT",
                    });
                    drop[significant[n + 3]] = true;
                    drop[significant[n + 4]] = true;
                }
            }
        }
    }
    if dialect.is_psql() {
        for n in 1..significant.len() {
            match word(n).as_str() {
                "MATCH" if matches!(word(n + 1).as_str(), "SIMPLE" | "FULL" | "PARTIAL") => {
                    drop[significant[n]] = true;
                    drop[significant[n + 1]] = true;
                }
                "NOT" if word(n + 1) == "VALID" => {
                    drop[significant[n]] = true;
                    drop[significant[n + 1]] = true;
                }
                // `varchar ARRAY` / `varchar(10) ARRAY` (not the ARRAY[...] / ARRAY(...)
                // constructors)
                "ARRAY"
                    if matches!(token(n - 1), Some(Token::Word(_) | Token::RParen))
                        && !matches!(token(n + 1), Some(Token::LBracket | Token::LParen)) =>
                {
                    if let Some(start) = offset(&tokens[significant[n]]) {
                        edits.push(super::warehouse::Edit {
                            range: start..start + "ARRAY".len(),
                            text: "[]",
                        });
                    }
                }
                _ => {}
            }
        }
    }
    // The column list: the first `(` after CREATE ... TABLE name
    let Some(table) = (0..significant.len()).find(|&n| word(n) == "TABLE") else {
        return edits;
    };
    let Some(open) = (table..significant.len()).find(|&n| is(n, &Token::LParen)) else {
        return edits;
    };
    // The elements of the list: (first, last) significant-token indexes
    let mut elements = Vec::new();
    let mut depth = 0usize;
    let mut start = open + 1;
    for n in open..significant.len() {
        match token(n) {
            Some(Token::LParen) => depth += 1,
            Some(Token::RParen) => {
                depth -= 1;
                if depth == 0 {
                    if n > start {
                        elements.push((start, n - 1));
                    }
                    break;
                }
            }
            Some(Token::Comma) if depth == 1 => {
                if n > start {
                    elements.push((start, n - 1));
                }
                start = n + 1;
            }
            _ => {}
        }
    }
    let mut mark = |first: usize, last: usize| {
        for &i in &significant[first..=last] {
            drop[i] = true;
        }
    };
    for (first, last) in elements {
        // Skip `CONSTRAINT [name]`
        let mut n = first;
        if word(n) == "CONSTRAINT" {
            n += 1;
            if !matches!(
                word(n).as_str(),
                "PRIMARY" | "UNIQUE" | "FOREIGN" | "CHECK" | "EXCLUDE"
            ) {
                n += 1;
            }
        }
        let kind = word(n);
        // `LIKE p` with other elements (or options) after it, which sqlparser only
        // parses as the whole list: the column [`like_pseudo_column`] recognizes
        if kind == "LIKE" && n == first {
            if let Some(start) = offset(&tokens[significant[n]]) {
                edits.push(super::warehouse::Edit {
                    range: start..start + "LIKE".len(),
                    text: LIKE_COLUMN,
                });
            }
            continue;
        }
        if dialect.is_psql() {
            if kind == "EXCLUDE" {
                // With the comma before it
                mark(first - 1, last);
            }
            continue;
        }
        match kind.as_str() {
            "PRIMARY" | "UNIQUE" | "KEY" | "INDEX" | "FULLTEXT" | "SPATIAL" => {
                // The key part list: the element's first `(`
                let Some(list) = (n..=last).find(|&k| is(k, &Token::LParen)) else {
                    continue;
                };
                let mut depth = 0usize;
                let mut k = list;
                while k <= last {
                    match token(k) {
                        Some(Token::LParen) => depth += 1,
                        Some(Token::RParen) => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                    if depth == 1 {
                        if matches!(word(k).as_str(), "ASC" | "DESC") {
                            mark(k, k);
                        } else if matches!(token(k), Some(Token::Word(_)))
                            && is(k + 1, &Token::LParen)
                            && matches!(token(k + 2), Some(Token::Number(..)))
                            && is(k + 3, &Token::RParen)
                        {
                            // A prefix length: `name(10)`
                            mark(k + 1, k + 3);
                            k += 3;
                        }
                    }
                    k += 1;
                }
            }
            "FOREIGN" if word(n + 1) == "KEY" => {
                if matches!(token(n + 2), Some(Token::Word(_))) && is(n + 3, &Token::LParen) {
                    mark(n + 2, n + 2);
                }
            }
            _ => {
                // A column: `name DOUBLE(M,D)`, `name FLOAT(M,D)`, `name REAL(M,D)`
                if matches!(word(n + 1).as_str(), "DOUBLE" | "FLOAT" | "REAL")
                    && is(n + 2, &Token::LParen)
                    && matches!(token(n + 3), Some(Token::Number(..)))
                    && is(n + 4, &Token::Comma)
                    && matches!(token(n + 5), Some(Token::Number(..)))
                    && is(n + 6, &Token::RParen)
                {
                    mark(n + 2, n + 6);
                }
            }
        }
    }
    edits
}

/// Kind (`CREATE TABLE` / `CREATE VIEW`) and name of the table or view a statement
/// that could not be parsed defines, if it is such a definition
pub(crate) fn unparsed_definition(dialect: SqlDialect, stmt: &str) -> Option<(String, String)> {
    let parser_dialect = dialect.parser_dialect();
    let tokens = Tokenizer::new(parser_dialect.as_ref(), stmt)
        .with_unescape(false)
        .tokenize()
        .ok()?;
    let significant: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let word = |i: usize| match significant.get(i) {
        Some(Token::Word(w)) if w.quote_style.is_none() => w.value.to_uppercase(),
        _ => String::new(),
    };
    if word(0) != "CREATE" {
        return None;
    }
    // Skip modifiers such as OR REPLACE, TEMP, UNLOGGED or MATERIALIZED
    let mut i = 1;
    while matches!(
        word(i).as_str(),
        "OR" | "REPLACE"
            | "TEMP"
            | "TEMPORARY"
            | "UNLOGGED"
            | "GLOBAL"
            | "LOCAL"
            | "MATERIALIZED"
            | "RECURSIVE"
    ) {
        i += 1;
    }
    let kind = match word(i).as_str() {
        "TABLE" => "CREATE TABLE",
        "VIEW" => "CREATE VIEW",
        _ => return None,
    };
    i += 1;
    if word(i) == "IF" && word(i + 1) == "NOT" && word(i + 2) == "EXISTS" {
        i += 3;
    }
    let name_tokens = significant.get(i..)?;
    let name = split_object_names(&name_tokens[..object_name_len(name_tokens)])
        .into_iter()
        .next()?;
    Some((kind.to_string(), name.to_string()))
}

/// Number of tokens forming the (possibly qualified) object name `a.b.c` at the
/// start of `tokens`
fn object_name_len(tokens: &[&Token]) -> usize {
    let mut len = 0;
    while matches!(tokens.get(len), Some(Token::Word(_))) {
        len += 1;
        if matches!(tokens.get(len), Some(Token::Period))
            && matches!(tokens.get(len + 1), Some(Token::Word(_)))
        {
            len += 1;
        } else {
            break;
        }
    }
    len
}

/// Split a token sequence like `a.b, "C", d` into object names, stopping at the
/// first token that isn't part of the list
fn split_object_names(tokens: &[&Token]) -> Vec<ObjectName> {
    let mut names = Vec::new();
    let mut rest = tokens;
    loop {
        let len = object_name_len(rest);
        if len == 0 {
            break;
        }
        let parts: Vec<Ident> = rest[..len]
            .iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(match w.quote_style {
                    Some(q) => Ident::with_quote(q, w.value.clone()),
                    None => Ident::new(w.value.clone()),
                }),
                _ => None,
            })
            .collect();
        names.push(ObjectName::from(parts));
        match rest.get(len) {
            Some(Token::Comma) => rest = &rest[len + 1..],
            _ => break,
        }
    }
    names
}

/// 1-indexed (line, column in characters) of a byte offset in `text`
fn line_column_at(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}

/// Convert a (line, column) relative to a statement starting at `base` into a
/// location in the whole input
fn absolute_location(base: (usize, usize), line: usize, column: usize) -> (usize, usize) {
    if line <= 1 {
        (base.0, base.1 + column.saturating_sub(1))
    } else {
        (base.0 + line - 1, column)
    }
}

/// Byte offset in `text` of a 1-indexed (line, column in characters)
fn byte_offset_of(text: &str, line: usize, column: usize) -> Option<usize> {
    let mut offset = 0;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            let in_line = l
                .char_indices()
                .nth(column.saturating_sub(1))
                .map_or(l.len(), |(b, _)| b);
            return Some(offset + in_line);
        }
        offset += l.len();
    }
    None
}

/// Rewrite the `at Line: X, Column: Y` suffix of a parser error message (relative
/// to the statement) into a location in the whole input
fn relocate_parser_message(message: &str, base: (usize, usize)) -> String {
    let message = message
        .strip_prefix("sql parser error: ")
        .unwrap_or(message);
    let Some(at) = message.rfind(" at Line: ") else {
        return message.to_string();
    };
    let location = &message[at + " at Line: ".len()..];
    let parsed = location.split_once(", Column: ").and_then(|(l, c)| {
        Some((
            l.trim().parse::<usize>().ok()?,
            c.trim().parse::<usize>().ok()?,
        ))
    });
    match parsed {
        Some((line, column)) => {
            let (line, column) = absolute_location(base, line, column);
            format!("{} at Line: {line}, Column: {column}", &message[..at])
        }
        None => message.to_string(),
    }
}

/// Convert expression to DefaultValue
fn expr_to_default(expr: &sqlparser::ast::Expr) -> DefaultValue {
    match expr {
        sqlparser::ast::Expr::Value(v) => match &v.value {
            sqlparser::ast::Value::Null => DefaultValue::Null,
            _ => DefaultValue::Literal(v.to_string()),
        },
        sqlparser::ast::Expr::Function(f) => {
            let func_name = f.name.to_string().to_lowercase();
            if func_name.contains("now") || func_name.contains("current_timestamp") {
                DefaultValue::CurrentTimestamp
            } else if func_name.contains("nextval") {
                DefaultValue::NextVal(f.to_string())
            } else {
                DefaultValue::Expression(f.to_string())
            }
        }
        _ => DefaultValue::Expression(expr.to_string()),
    }
}

/// Split SQL text into individual statements by semicolons, skipping semicolons
/// inside string literals, quoted identifiers, dollar-quoted bodies and comments.
///
/// Quoting rules follow the dialect: MySQL strings use backslash escapes and
/// backtick identifiers (as SQLite allows too), PostgreSQL only has backslash
/// escapes in `E'...'` strings, supports dollar quoting and nests block comments.
fn split_sql_statements(sql: &str, dialect: SqlDialect) -> Vec<&str> {
    let mysql = dialect == SqlDialect::MySQL;
    let postgres = dialect == SqlDialect::PostgreSQL;
    let backticks = dialect.backtick_identifiers();

    let mut statements = Vec::new();
    let mut start = 0;
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80;

    // Skip a quoted section starting at `i` (the opening quote); a doubled quote
    // is an escaped quote, and with `backslash` a backslash escapes the next byte
    let skip_quoted = |mut i: usize, quote: u8, backslash: bool| -> usize {
        i += 1;
        while i < len {
            if backslash && bytes[i] == b'\\' {
                i += 2;
            } else if bytes[i] == quote {
                i += 1;
                if i < len && bytes[i] == quote {
                    i += 1; // escaped quote
                } else {
                    return i;
                }
            } else {
                i += 1;
            }
        }
        len
    };

    while i < len {
        match bytes[i] {
            b'\'' => {
                // PostgreSQL E'...' escape strings honor backslash escapes
                let escape_string = postgres
                    && i > 0
                    && matches!(bytes[i - 1], b'E' | b'e')
                    && (i < 2 || !is_ident_byte(bytes[i - 2]));
                i = skip_quoted(i, b'\'', mysql || escape_string);
            }
            b'"' => {
                // Quoted identifier (a string in MySQL)
                i = skip_quoted(i, b'"', mysql);
            }
            b'`' if backticks => {
                i = skip_quoted(i, b'`', false);
            }
            b'$' if postgres && (i == 0 || !is_ident_byte(bytes[i - 1])) => {
                // Check for dollar-quoted string ($$...$$ or $tag$...$tag$)
                if let Some(tag_end) = find_dollar_tag_end(sql, i) {
                    let tag = &sql[i..=tag_end];
                    i = tag_end + 1;
                    // Find the closing tag
                    if let Some(close_pos) = sql[i..].find(tag) {
                        i += close_pos + tag.len();
                    } else {
                        i = len; // unterminated, consume rest
                    }
                } else {
                    i += 1;
                }
            }
            b'-' if i + 1 < len && bytes[i + 1] == b'-' => {
                // Skip line comment
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' if mysql => {
                // MySQL line comment
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                // Skip block comment (PostgreSQL block comments nest)
                i += 2;
                let mut depth = 1;
                while i < len && depth > 0 {
                    if bytes[i] == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else if postgres && bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b';' => {
                let stmt = &sql[start..i];
                if !stmt.trim().is_empty() {
                    statements.push(stmt);
                }
                start = i + 1;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Handle last statement (without trailing semicolon)
    let last = &sql[start..len.max(start)];
    if !last.trim().is_empty() {
        statements.push(last);
    }

    statements
}

/// Find the end of a dollar-quote tag starting at position `start`.
/// Returns the index of the closing `$` if a valid tag is found.
fn find_dollar_tag_end(sql: &str, start: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    // Tag is $<identifier>$ or just $$
    let mut i = start + 1;
    if i < len && bytes[i] == b'$' {
        return Some(i); // $$ tag
    }
    // Look for $identifier$ (tags can't start with a digit: `$1` is a parameter)
    if i < len && bytes[i].is_ascii_digit() {
        return None;
    }
    while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i < len && bytes[i] == b'$' {
        Some(i)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_table() {
        let sql = r"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email TEXT UNIQUE,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        let table = catalog.get_table(&QualifiedName::new("users")).unwrap();
        assert_eq!(table.columns.len(), 4);

        let id_col = table.get_column("id").unwrap();
        assert!(!id_col.nullable);
        assert!(id_col.is_primary_key);

        let name_col = table.get_column("name").unwrap();
        assert!(!name_col.nullable);
        assert!(matches!(name_col.data_type, SqlType::Varchar { .. }));

        let email_col = table.get_column("email").unwrap();
        assert!(email_col.nullable);
    }

    #[test]
    fn test_parse_table_with_foreign_key() {
        let sql = r"
            CREATE TABLE orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id),
                total DECIMAL(10, 2)
            );
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        let table = catalog.get_table(&QualifiedName::new("orders")).unwrap();
        assert_eq!(table.columns.len(), 3);
    }

    #[test]
    fn test_split_sql_statements() {
        let sql = "CREATE TABLE a (id INT); CREATE TABLE b (id INT);";
        let stmts = split_sql_statements(sql, SqlDialect::PostgreSQL);
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn test_split_preserves_string_literals() {
        let sql = "SELECT 'hello; world'; CREATE TABLE t (id INT);";
        let stmts = split_sql_statements(sql, SqlDialect::PostgreSQL);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("hello; world"));
    }

    #[test]
    fn test_split_quoted_identifiers_and_comments() {
        let sql = "CREATE TABLE \"a;b\" (x int); -- c;d\nSELECT 1 /* e; /* f; */ g; */; SELECT 2";
        let stmts = split_sql_statements(sql, SqlDialect::PostgreSQL);
        assert_eq!(stmts.len(), 3, "{stmts:?}");
        assert!(stmts[0].contains("\"a;b\""));
    }

    #[test]
    fn test_split_backslash_escapes_depend_on_dialect() {
        // MySQL: backslash escapes the quote
        let sql = "SELECT 'a\\';b'; SELECT `c;d`; # e;f\nSELECT 3";
        let stmts = split_sql_statements(sql, SqlDialect::MySQL);
        assert_eq!(stmts.len(), 3, "{stmts:?}");

        // PostgreSQL: backslash is literal in standard strings, an escape in E''
        let sql = "SELECT 'a\\'; SELECT E'b\\';c'; SELECT $1, $tag$ x; $tag$";
        let stmts = split_sql_statements(sql, SqlDialect::PostgreSQL);
        assert_eq!(stmts.len(), 3, "{stmts:?}");
        assert!(stmts[1].contains("E'b\\';c'"));
    }

    #[test]
    fn test_relocate_parser_message() {
        assert_eq!(
            relocate_parser_message(
                "sql parser error: Expected: x, found: y at Line: 1, Column: 5",
                (3, 4)
            ),
            "Expected: x, found: y at Line: 3, Column: 8"
        );
        assert_eq!(
            relocate_parser_message("Expected: x at Line: 2, Column: 5", (3, 4)),
            "Expected: x at Line: 4, Column: 5"
        );
    }

    #[test]
    fn test_parse_with_unsupported_statements() {
        let sql = r"
            CREATE OR REPLACE PROCEDURAL LANGUAGE plpgsql;

            CREATE TABLE actor (
                actor_id integer NOT NULL,
                first_name character varying(45) NOT NULL
            );

            ALTER TABLE public.actor OWNER TO postgres;

            CREATE TABLE category (
                category_id integer NOT NULL,
                name character varying(25) NOT NULL
            );
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        // Both tables should be found despite the unsupported PROCEDURAL LANGUAGE
        assert!(catalog.table_exists(&QualifiedName::new("actor")));
        assert!(catalog.table_exists(&QualifiedName::new("category")));
    }

    #[test]
    fn test_parse_sakila_like_schema() {
        // Simulates Sakila-style schema with mixed supported/unsupported statements
        let sql = r"
            SET client_encoding = 'UTF8';
            SET standard_conforming_strings = off;

            COMMENT ON SCHEMA public IS 'Standard public schema';

            CREATE SEQUENCE actor_actor_id_seq
                INCREMENT BY 1
                NO MAXVALUE
                NO MINVALUE
                CACHE 1;

            CREATE TABLE actor (
                actor_id integer DEFAULT nextval('actor_actor_id_seq'::regclass) NOT NULL,
                first_name character varying(45) NOT NULL,
                last_name character varying(45) NOT NULL,
                last_update timestamp without time zone DEFAULT now() NOT NULL
            );

            ALTER TABLE public.actor OWNER TO postgres;

            CREATE TYPE mpaa_rating AS ENUM (
                'G', 'PG', 'PG-13', 'R', 'NC-17'
            );

            CREATE TABLE film (
                film_id integer NOT NULL,
                title character varying(255) NOT NULL,
                description text,
                release_year integer,
                rental_rate numeric(4,2) DEFAULT 4.99 NOT NULL,
                rating mpaa_rating DEFAULT 'G'
            );

            CREATE TABLE category (
                category_id integer NOT NULL,
                name character varying(25) NOT NULL,
                last_update timestamp without time zone DEFAULT now() NOT NULL
            );
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        assert!(
            catalog.table_exists(&QualifiedName::new("actor")),
            "actor table should exist"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("film")),
            "film table should exist"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("category")),
            "category table should exist"
        );
        assert!(
            catalog.enum_exists("mpaa_rating"),
            "mpaa_rating enum should exist"
        );
    }

    #[test]
    fn test_parse_with_functions_and_triggers() {
        let sql = r"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE FUNCTION update_timestamp() RETURNS TRIGGER AS $$
            BEGIN
                NEW.updated_at = NOW();
                RETURN NEW;
            END;
            $$ LANGUAGE plpgsql;

            CREATE TABLE posts (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL,
                user_id INTEGER NOT NULL
            );
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, _) = builder.build();

        assert!(catalog.table_exists(&QualifiedName::new("users")));
        assert!(catalog.table_exists(&QualifiedName::new("posts")));
    }

    #[test]
    fn test_drop_table() {
        let sql = r"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE posts (
                id SERIAL PRIMARY KEY,
                title TEXT NOT NULL
            );

            DROP TABLE users;
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, warnings) = builder.build();

        assert!(
            !catalog.table_exists(&QualifiedName::new("users")),
            "users table should be removed after DROP TABLE"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("posts")),
            "posts table should still exist"
        );
        assert!(warnings.is_empty(), "no warnings should be produced");
    }

    #[test]
    fn test_drop_table_if_exists() {
        let sql = r"
            CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL
            );

            DROP TABLE IF EXISTS users;
            DROP TABLE IF EXISTS nonexistent;
        ";

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, warnings) = builder.build();

        assert!(
            !catalog.table_exists(&QualifiedName::new("users")),
            "users table should be removed"
        );
        assert!(warnings.is_empty(), "no warnings should be produced");
    }

    #[test]
    fn test_drop_table_then_alter_produces_no_warning() {
        // Simulates the Prisma migration pattern: drop old tables, create new ones,
        // then ALTER TABLE the new tables. Should produce no spurious warnings.
        let sql = r#"
            CREATE TABLE old_items (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE new_items (
                id UUID PRIMARY KEY,
                label TEXT NOT NULL
            );

            ALTER TABLE old_items DROP CONSTRAINT "old_items_pkey";
            DROP TABLE old_items;

            ALTER TABLE new_items ADD COLUMN description TEXT;
        "#;

        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        let (catalog, warnings) = builder.build();

        assert!(
            !catalog.table_exists(&QualifiedName::new("old_items")),
            "old_items should be dropped"
        );
        assert!(
            catalog.table_exists(&QualifiedName::new("new_items")),
            "new_items should exist"
        );
        assert!(warnings.is_empty(), "no warnings should be produced");
    }
}
