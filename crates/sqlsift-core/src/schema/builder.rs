//! Schema builder - converts SQL AST to Catalog

use sqlparser::ast::{
    AlterColumnOperation, AlterTableOperation, ColumnOption, ColumnOptionDef, DataType, Ident,
    ObjectName, ObjectType, Query, Statement, TableConstraint, UserDefinedTypeRepresentation,
};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use crate::analyzer;
use crate::dialect::SqlDialect;
use crate::error::{Diagnostic, DiagnosticKind, Span};
use crate::psql;
use crate::schema::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, IdentityKind,
    PrimaryKeyDef, QualifiedName, SkippedDefinition, TableDef, UniqueConstraintDef, ViewDef,
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
}

impl SchemaBuilder {
    pub fn new() -> Self {
        Self::with_dialect(SqlDialect::default())
    }

    pub fn with_dialect(dialect: SqlDialect) -> Self {
        let mut catalog = Catalog::new();
        catalog.case_sensitive_names = dialect == SqlDialect::PostgreSQL;
        Self {
            catalog,
            diagnostics: Vec::new(),
            dialect,
            query_file: false,
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
        }
    }

    /// Apply one parsed statement to the catalog (statements that don't define or
    /// change the schema are ignored)
    pub(crate) fn apply_statement(&mut self, stmt: &Statement) {
        self.process_statement(stmt);
    }

    /// Whether `stmt` is a statement [`SchemaBuilder`] applies to the catalog
    pub(crate) fn changes_schema(stmt: &Statement) -> bool {
        matches!(
            stmt,
            Statement::CreateTable(_)
                | Statement::CreateType { .. }
                | Statement::CreateView { .. }
                | Statement::AlterTable { .. }
                | Statement::Drop {
                    object_type: ObjectType::Table | ObjectType::View | ObjectType::Type,
                    ..
                }
        )
    }

    /// Parse SQL schema definitions and build the catalog.
    ///
    /// dbmate `-- migrate:down` sections are ignored (see
    /// [`strip_down_migrations`](crate::schema::strip_down_migrations)).
    pub fn parse(&mut self, sql: &str) -> Result<(), Vec<Diagnostic>> {
        let sql = &*crate::schema::strip_down_migrations(sql);
        let dialect = self.dialect.parser_dialect();

        // psql meta-commands in dumps and scripts (`\connect`, `\restrict`, `\i`, ...)
        let source = match self.dialect {
            SqlDialect::PostgreSQL => psql::preprocess(sql),
            SqlDialect::MySQL | SqlDialect::SQLite => psql::Preprocessed::unchanged(sql),
        };
        let sql: &str = &source.text;

        // Try parsing the entire SQL first (fast path)
        match Parser::parse_sql(dialect.as_ref(), sql) {
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
            match Parser::parse_sql(dialect.as_ref(), raw_stmt) {
                Ok(stmts) => {
                    for stmt in stmts {
                        self.process_statement(&stmt);
                    }
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
        let Ok(tokens) = Tokenizer::new(parser_dialect.as_ref(), stmt)
            .with_unescape(false)
            .tokenize_with_location()
        else {
            return;
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
            match object {
                Some((Some(kind), name_start)) => (kind, name_start),
                _ => return,
            }
        } else if word(0) == "ALTER" && word(1) == "TABLE" {
            ("ALTER TABLE", 2)
        } else {
            return;
        };

        // Retry `CREATE TABLE c (LIKE p INCLUDING ...)` / `... WITH NO DATA` without
        // those clauses
        if matches!(kind, "CREATE TABLE" | "CREATE VIEW")
            && self.retry_without_unsupported_clauses(&tokens)
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
                    !matches!(word(op + 1), "CONSTRAINT" | "INDEX" | "KEY")
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

    /// Re-parse a CREATE TABLE / CREATE VIEW statement without clauses that don't
    /// affect columns and that sqlparser can't parse: `LIKE p INCLUDING x` /
    /// `EXCLUDING x` options, and a trailing `WITH [NO] DATA`. Returns true if
    /// something was removed and the statement then parsed and was applied.
    fn retry_without_unsupported_clauses(&mut self, tokens: &[TokenWithSpan]) -> bool {
        let is_word = |t: &Token, kw: &str| matches!(t, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(kw));
        // Indexes (into `tokens`) of significant tokens, and which ones to drop
        let significant: Vec<usize> = (0..tokens.len())
            .filter(|&i| !matches!(tokens[i].token, Token::Whitespace(_)))
            .collect();
        let mut drop = vec![false; tokens.len()];
        for (n, &i) in significant.iter().enumerate() {
            if is_word(&tokens[i].token, "INCLUDING") || is_word(&tokens[i].token, "EXCLUDING") {
                if let Some(&next) = significant.get(n + 1) {
                    if matches!(tokens[next].token, Token::Word(_)) {
                        drop[i] = true;
                        drop[next] = true;
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
        // PostgreSQL `INHERITS (parent [, ...])`: parsed separately, applied below
        let mut parents = Vec::new();
        if let Some(n) = significant
            .iter()
            .position(|&i| is_word(&tokens[i].token, "INHERITS"))
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
        if !drop.contains(&true) {
            return false;
        }

        let rewritten: String = tokens
            .iter()
            .zip(&drop)
            .filter(|(_, &dropped)| !dropped)
            .map(|(t, _)| t.token.to_string())
            .collect();
        let dialect = self.dialect.parser_dialect();
        let Ok(stmts) = Parser::parse_sql(dialect.as_ref(), &rewritten) else {
            return false;
        };
        for stmt in stmts {
            self.process_statement(&stmt);
            if let Statement::CreateTable(create) = &stmt {
                if !parents.is_empty() {
                    let child = self.catalog.qualified_name(&create.name);
                    self.inherit_columns(&child, &parents);
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
        table
            .check_constraints
            .clone_from(&parent_table.check_constraints);
        self.catalog.add_table(table);
        true
    }

    /// Apply `ALTER TYPE name ADD VALUE ...` / `RENAME VALUE ...` / `RENAME TO ...`
    /// (not parsed by sqlparser) to an enum. `tokens` start after `ALTER TYPE`.
    fn process_alter_type_tokens(&mut self, tokens: &[&Token]) {
        let name_len = object_name_len(tokens);
        let Some(name) = split_object_names(&tokens[..name_len]).into_iter().next() else {
            return;
        };
        let enum_name = self.catalog.qualified_name(&name).name;
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
                representation,
            } => {
                self.process_create_type(name, representation);
            }
            Statement::CreateView {
                name,
                columns,
                query,
                materialized,
                if_not_exists,
                ..
            } => {
                let qualified = self.catalog.qualified_name(name);
                if *if_not_exists && self.relation_exists(&qualified) {
                    return;
                }
                self.process_create_view(qualified, columns, query, *materialized);
            }
            Statement::AlterTable {
                name, operations, ..
            } => {
                self.process_alter_table(name, operations);
            }
            Statement::Drop {
                object_type, names, ..
            } => {
                for name in names {
                    match object_type {
                        ObjectType::Table => self.process_drop_table(name),
                        ObjectType::View => {
                            let name = self.catalog.qualified_name(name);
                            self.catalog.drop_view(&name);
                        }
                        ObjectType::Type => {
                            let name = self.catalog.qualified_name(name);
                            self.catalog.drop_enum(&name.name);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether a table or view with this name exists
    fn relation_exists(&self, name: &QualifiedName) -> bool {
        self.catalog.table_exists(name) || self.catalog.view_exists(name)
    }

    /// Process CREATE TABLE statement
    fn process_create_table(&mut self, create: &sqlparser::ast::CreateTable) {
        let name = self.catalog.qualified_name(&create.name);
        if create.if_not_exists && self.relation_exists(&name) {
            return;
        }
        let mut table = TableDef::new(name.clone());

        // MySQL `CREATE TABLE t2 LIKE t`: copy the definition
        if let Some(like) = create.like.as_ref().or(create.clone.as_ref()) {
            let source = self.catalog.qualified_name(like);
            match self.catalog.get_table(&source) {
                Some(source_table) => {
                    table.columns.clone_from(&source_table.columns);
                    table.primary_key.clone_from(&source_table.primary_key);
                    table
                        .unique_constraints
                        .clone_from(&source_table.unique_constraints);
                    table
                        .check_constraints
                        .clone_from(&source_table.check_constraints);
                }
                None => self.warn_like_source_missing(&name, &source),
            }
        }

        // Process columns
        for column in &create.columns {
            // `CREATE TABLE c (LIKE p)` parses as a column named LIKE of type `p`
            if let Some(source) = like_pseudo_column(column) {
                self.copy_like_columns(&mut table, source);
                continue;
            }

            let (col_def, constraints) = build_column(
                &self.catalog,
                &name.name,
                &column.name.value,
                &column.data_type,
                &column.options,
            );
            merge_constraints(&mut table, constraints);
            table.columns.insert(col_def.name.clone(), col_def);
        }

        // CREATE TABLE ... AS SELECT: infer column names from the query
        if let Some(query) = &create.query {
            if create.columns.is_empty() {
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
                        self.catalog.drop_table(&name);
                        self.catalog.add_view(ViewDef {
                            name,
                            columns: Vec::new(),
                            column_types: Vec::new(),
                            materialized: false,
                        });
                        return;
                    }
                    None => self.diagnostics.push(Diagnostic::warning(
                        DiagnosticKind::ParseError,
                        format!(
                            "Could not determine the columns of table '{name}' created by CREATE TABLE ... AS"
                        ),
                    ).with_help("Queries that reference its columns may report missing columns")),
                }
            }
        }

        // Process table constraints
        for constraint in &create.constraints {
            self.process_table_constraint(&mut table, constraint);
        }

        self.catalog.add_table(table);
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
                    | AlterTableOperation::AddConstraint(_)
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
            self.diagnostics.push(
                Diagnostic::warning(
                    DiagnosticKind::TableNotFound,
                    format!(
                        "ALTER TABLE references table '{table_name}' which was not found in schema"
                    ),
                )
                .with_help("Ensure the CREATE TABLE statement appears before ALTER TABLE"),
            );
            return;
        }

        for operation in operations {
            match operation {
                AlterTableOperation::AddColumn { column_def, .. } => {
                    let (col, constraints) = build_column(
                        &self.catalog,
                        &table_name.name,
                        &column_def.name.value,
                        &column_def.data_type,
                        &column_def.options,
                    );
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        merge_constraints(table, constraints);
                        table.forget_former_column(&col.name);
                        table.columns.insert(col.name.clone(), col);
                    }
                }
                AlterTableOperation::DropColumn { column_name, .. } => {
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        if let Some(index) = column_index(table, &column_name.value) {
                            if let Some((name, _)) = table.columns.shift_remove_index(index) {
                                table.record_column_drop(&name);
                            }
                        }
                    }
                }
                AlterTableOperation::RenameColumn {
                    old_column_name,
                    new_column_name,
                } => {
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        if let Some(index) = column_index(table, &old_column_name.value) {
                            let mut col = table.columns[index].clone();
                            col.name.clone_from(&new_column_name.value);
                            let old_name = table.columns[index].name.clone();
                            table.record_column_rename(&old_name, &col.name);
                            replace_column(table, index, col);
                        }
                    }
                }
                AlterTableOperation::AlterColumn { column_name, op } => {
                    if let Some(col) = self.catalog.get_table_mut(&table_name).and_then(|t| {
                        column_index(t, &column_name.value).map(|i| &mut t.columns[i])
                    }) {
                        apply_alter_column(col, op);
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
                        &table_name.name,
                        &new_name.value,
                        data_type,
                        &options,
                    );
                    if let Some(table) = self.catalog.get_table_mut(&table_name) {
                        merge_constraints(table, constraints);
                        if let Some(index) = column_index(table, &col_name.value) {
                            let old_name = table.columns[index].name.clone();
                            table.record_column_rename(&old_name, &col.name);
                            replace_column(table, index, col);
                        } else {
                            table.forget_former_column(&col.name);
                            table.columns.insert(col.name.clone(), col);
                        }
                    }
                }
                AlterTableOperation::RenameTable {
                    table_name: new_name,
                } => {
                    let new_qualified = self.catalog.qualified_name(new_name);
                    let schema_name = table_name
                        .schema
                        .as_ref()
                        .unwrap_or(&self.catalog.default_schema);
                    if let Some(schema) = self.catalog.schemas.get_mut(schema_name) {
                        if let Some(mut table) = schema.tables.shift_remove(&table_name.name) {
                            table.name = new_qualified.clone();
                            schema.tables.insert(new_qualified.name, table);
                        }
                    }
                }
                AlterTableOperation::AddConstraint(constraint) => {
                    let mut constraints = TableDef::new(table_name.clone());
                    self.process_table_constraint(&mut constraints, constraint);
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
                        merge_constraints(table, constraints);
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
        let table_name = self.catalog.qualified_name(name);
        self.catalog.drop_table(&table_name);
        if self.query_file {
            // A `CREATE TABLE ... AS` with unknown columns is defined as a view
            self.catalog.drop_view(&table_name);
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
            let enum_def = EnumTypeDef {
                name: qualified.name,
                values: labels.iter().map(|l| l.value.clone()).collect(),
            };
            self.catalog.add_enum(enum_def);
        } else {
            // Composite types and others - not yet supported
        }
    }

    /// Process a table constraint (PRIMARY KEY, FOREIGN KEY, UNIQUE)
    fn process_table_constraint(&mut self, table: &mut TableDef, constraint: &TableConstraint) {
        match constraint {
            TableConstraint::PrimaryKey { columns, name, .. } => {
                let pk = PrimaryKeyDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
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
            TableConstraint::ForeignKey {
                columns,
                foreign_table,
                referred_columns,
                name,
                ..
            } => {
                let fk = ForeignKeyDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                    references_table: self.catalog.qualified_name(foreign_table),
                    references_columns: referred_columns.iter().map(|c| c.value.clone()).collect(),
                };
                table.foreign_keys.push(fk);
            }
            TableConstraint::Unique { columns, name, .. } => {
                let unique = UniqueConstraintDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    columns: columns.iter().map(|c| c.value.clone()).collect(),
                };
                table.unique_constraints.push(unique);
            }
            TableConstraint::Check { name, expr, .. } => {
                let check = CheckConstraintDef {
                    name: name.as_ref().map(|n| n.value.clone()),
                    expression: expr.to_string(),
                };
                table.check_constraints.push(check);
            }
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

/// Build a column definition from its name, type and options. Table-level
/// constraints declared inline (PRIMARY KEY, UNIQUE, REFERENCES, CHECK) are
/// collected into the returned `TableDef`, to be merged into the owning table.
fn build_column(
    catalog: &Catalog,
    table_name: &str,
    col_name: &str,
    data_type: &DataType,
    options: &[ColumnOptionDef],
) -> (ColumnDef, TableDef) {
    let mut col = ColumnDef::new(col_name, SqlType::from_ast(data_type));
    let mut constraints = TableDef::new(QualifiedName::new(table_name));

    for option in options {
        let constraint_name = option.name.as_ref().map(|n| n.value.clone());
        match &option.option {
            ColumnOption::Null => col.nullable = true,
            ColumnOption::NotNull => col.nullable = false,
            ColumnOption::Default(expr) => col.default = Some(expr_to_default(expr)),
            ColumnOption::Unique { is_primary, .. } => {
                if *is_primary {
                    col.is_primary_key = true;
                    col.nullable = false;
                    constraints.primary_key = Some(PrimaryKeyDef {
                        name: constraint_name,
                        columns: vec![col_name.to_string()],
                    });
                } else {
                    constraints.unique_constraints.push(UniqueConstraintDef {
                        name: constraint_name,
                        columns: vec![col_name.to_string()],
                    });
                }
            }
            ColumnOption::ForeignKey {
                foreign_table,
                referred_columns,
                ..
            } => {
                constraints.foreign_keys.push(ForeignKeyDef {
                    name: constraint_name,
                    columns: vec![col_name.to_string()],
                    references_table: catalog.qualified_name(foreign_table),
                    references_columns: referred_columns.iter().map(|c| c.value.clone()).collect(),
                });
            }
            ColumnOption::Check(expr) => {
                constraints.check_constraints.push(CheckConstraintDef {
                    name: constraint_name,
                    expression: expr.to_string(),
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

/// Whether a data type is one of PostgreSQL's serial pseudo-types
fn is_serial_type(data_type: &DataType) -> bool {
    match data_type {
        DataType::Custom(name, modifiers) if modifiers.is_empty() => {
            matches!(
                name.0.last().map(|i| i.value.to_lowercase()).as_deref(),
                Some("serial" | "serial2" | "serial4" | "serial8" | "smallserial" | "bigserial")
            )
        }
        _ => false,
    }
}

/// Add the constraints collected in `constraints` to `table`
fn merge_constraints(table: &mut TableDef, constraints: TableDef) {
    if constraints.primary_key.is_some() {
        table.primary_key = constraints.primary_key;
    }
    table.foreign_keys.extend(constraints.foreign_keys);
    table
        .unique_constraints
        .extend(constraints.unique_constraints);
    table
        .check_constraints
        .extend(constraints.check_constraints);
}

/// Apply an `ALTER TABLE ... ALTER COLUMN` operation to a column
fn apply_alter_column(col: &mut ColumnDef, op: &AlterColumnOperation) {
    match op {
        AlterColumnOperation::SetNotNull => col.nullable = false,
        AlterColumnOperation::DropNotNull => col.nullable = true,
        AlterColumnOperation::SetDefault { value } => col.default = Some(expr_to_default(value)),
        AlterColumnOperation::DropDefault => col.default = None,
        AlterColumnOperation::SetDataType { data_type, .. } => {
            col.data_type = SqlType::from_ast(data_type);
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

/// If `column` is the pseudo-column sqlparser produces for `LIKE p` inside a
/// CREATE TABLE column list, return the source table name
fn like_pseudo_column(column: &sqlparser::ast::ColumnDef) -> Option<&ObjectName> {
    if column.name.quote_style.is_some()
        || !column.name.value.eq_ignore_ascii_case("LIKE")
        || !column.options.is_empty()
    {
        return None;
    }
    match &column.data_type {
        DataType::Custom(name, modifiers) if modifiers.is_empty() => Some(name),
        _ => None,
    }
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
        let parts = rest[..len]
            .iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(match w.quote_style {
                    Some(q) => Ident::with_quote(q, w.value.clone()),
                    None => Ident::new(w.value.clone()),
                }),
                _ => None,
            })
            .collect();
        names.push(ObjectName(parts));
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
        sqlparser::ast::Expr::Value(v) => match v {
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
    let backticks = matches!(dialect, SqlDialect::MySQL | SqlDialect::SQLite);

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
