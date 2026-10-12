//! Lowering of schema statements (`CREATE TABLE / VIEW / TYPE / INDEX`, `ALTER TABLE`,
//! `DROP`).

use sqlparser::ast::helpers::stmt_create_table::CreateTableBuilder;
use sqlparser::ast::{
    AlterColumnOperation, AlterTable, AlterTableOperation, CheckConstraint, ColumnDef,
    ColumnOption, ColumnOptionDef, ConstraintCharacteristics, ConstraintReferenceMatchKind,
    CreateIndex, CreateTableOptions, CreateView, DeferrableInitial, DropBehavior, Expr,
    ForeignKeyConstraint, GeneratedAs, GeneratedExpressionMode, Ident, IndexColumn, IndexType,
    KeyOrIndexDisplay, NullsDistinctOption, ObjectType, OnCommit, OrderByExpr, OrderByOptions,
    Owner, PrimaryKeyConstraint, ReferentialAction, RenameTableNameKind, Statement,
    TableConstraint, UniqueConstraint, UserDefinedTypeCompositeAttributeDef,
    UserDefinedTypeRepresentation, ViewColumnDef, WithData,
};

use super::{object_name, unquote, unsupported, Lowering, Result};
use crate::{SyntaxKind, SyntaxNode};

/// Child nodes of a constraint that are not its expression.
const CONSTRAINT_PARTS: &[SyntaxKind] = &[
    SyntaxKind::Name,
    SyntaxKind::ColumnList,
    SyntaxKind::ConstraintAttribute,
    SyntaxKind::IndexParams,
    SyntaxKind::ReferencesClause,
    SyntaxKind::SequenceOptions,
];

impl Lowering<'_> {
    /// The keywords (unquoted identifiers) directly in a node, lowercased.
    fn keywords(&self, node: &SyntaxNode) -> Vec<String> {
        node.tokens()
            .filter(|t| t.kind == SyntaxKind::Ident)
            .map(|t| self.text(t).to_ascii_lowercase())
            .collect()
    }

    pub(super) fn create_table(&self, node: &SyntaxNode) -> Result<Statement> {
        let kws = self.keywords(node);
        let has = |kw: &str| kws.iter().any(|k| k == kw);
        let Some(name) = node.child(SyntaxKind::Name) else {
            return unsupported("table name");
        };
        let mut builder = CreateTableBuilder::new(object_name(self.name_parts(name)?))
            .temporary(has("temp") || has("temporary"))
            .unlogged(has("unlogged"))
            .global(if has("global") {
                Some(true)
            } else if has("local") {
                Some(false)
            } else {
                None
            })
            .if_not_exists(has("exists"));
        if node.child(SyntaxKind::ColumnList).is_some() {
            return unsupported("CREATE TABLE ... AS column list");
        }
        if let Some(elements) = node.child(SyntaxKind::TableElements) {
            let mut columns = vec![];
            let mut constraints = vec![];
            for element in elements.nodes() {
                match element.kind {
                    SyntaxKind::ColumnDef => columns.push(self.column_def(element)?),
                    SyntaxKind::TableConstraint if !self.not_valid(element) => {
                        constraints.push(self.table_constraint(element)?);
                    }
                    kind => return unsupported(format!("{kind:?}")),
                }
            }
            builder = builder.columns(columns).constraints(constraints);
        }
        if let Some(query) = node.child(SyntaxKind::SelectStmt) {
            builder = builder.query(Some(Box::new(self.query(query)?)));
        }
        for option in node.nodes().filter(|n| n.kind == SyntaxKind::TableOption) {
            builder = self.table_option(builder, option)?;
        }
        Ok(Statement::CreateTable(builder.build()))
    }

    fn table_option(
        &self,
        builder: CreateTableBuilder,
        option: &SyntaxNode,
    ) -> Result<CreateTableBuilder> {
        let words = self.keywords(option);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let has_query = builder.query.is_some();
        Ok(match words[..] {
            ["with", "data"] if has_query && builder.with_data.is_none() => {
                builder.with_data(Some(WithData {
                    data: true,
                    statistics: None,
                }))
            }
            ["with", "no", "data"] if has_query && builder.with_data.is_none() => builder
                .with_data(Some(WithData {
                    data: false,
                    statistics: None,
                })),
            ["on", "commit", ref action @ ..] if builder.on_commit.is_none() => {
                builder.on_commit(Some(match action {
                    ["drop"] => OnCommit::Drop,
                    ["delete", "rows"] => OnCommit::DeleteRows,
                    ["preserve", "rows"] => OnCommit::PreserveRows,
                    _ => return unsupported("ON COMMIT"),
                }))
            }
            ["inherits"] if builder.inherits.is_none() => {
                let parents = option
                    .nodes()
                    .map(|n| Ok(object_name(self.name_parts(n)?)))
                    .collect::<Result<_>>()?;
                builder.inherits(Some(parents))
            }
            ["partition", "by"] if builder.partition_by.is_none() => {
                let Some(expr) = option.nodes().next() else {
                    return unsupported("PARTITION BY");
                };
                builder.partition_by(Some(Box::new(self.expr(expr)?)))
            }
            _ => {
                return unsupported(format!(
                    "table option {}",
                    words.first().copied().unwrap_or("")
                ))
            }
        })
    }

    fn column_def(&self, node: &SyntaxNode) -> Result<ColumnDef> {
        let Some(name) = node.tokens().next() else {
            return unsupported("column name");
        };
        let Some(data_type) = node.child(SyntaxKind::TypeName) else {
            return unsupported("column type");
        };
        let options = node
            .nodes()
            .filter(|n| n.kind == SyntaxKind::ColumnConstraint)
            .map(|n| self.column_option(n))
            .collect::<Result<_>>()?;
        Ok(ColumnDef {
            name: self.ident(name)?,
            data_type: self.data_type(data_type)?,
            options,
        })
    }

    /// The constraint name (`CONSTRAINT name`) and the keywords after it.
    fn constraint_head(&self, node: &SyntaxNode) -> Result<(Option<Ident>, Vec<String>)> {
        let mut kws = self.keywords(node);
        if kws.first().map(String::as_str) != Some("constraint") {
            return Ok((None, kws));
        }
        kws.remove(0);
        let name = node
            .child(SyntaxKind::Name)
            .and_then(|n| n.tokens().next())
            .map(|t| self.ident(t))
            .transpose()?;
        Ok((name, kws))
    }

    /// The expression of a `CHECK`, `DEFAULT` or generated column.
    fn constraint_expr(&self, node: &SyntaxNode) -> Result<Expr> {
        match node.nodes().find(|n| !CONSTRAINT_PARTS.contains(&n.kind)) {
            Some(expr) => self.expr(expr),
            None => unsupported("constraint without an expression"),
        }
    }

    fn column_option(&self, node: &SyntaxNode) -> Result<ColumnOptionDef> {
        if self.not_valid(node) {
            return unsupported("NOT VALID");
        }
        let (name, kws) = self.constraint_head(node)?;
        let characteristics = self.characteristics(node)?;
        let kws: Vec<&str> = kws.iter().map(String::as_str).collect();
        let option = match kws[..] {
            ["collate", ..] => {
                let Some(collation) = node.child(SyntaxKind::Name) else {
                    return unsupported("collation");
                };
                ColumnOption::Collation(object_name(self.name_parts(collation)?))
            }
            ["not", "null", ..] if characteristics.is_none() => ColumnOption::NotNull,
            ["null", ..] if characteristics.is_none() => ColumnOption::Null,
            ["default", ..] if characteristics.is_none() => {
                ColumnOption::Default(self.constraint_expr(node)?)
            }
            ["check", ..] if characteristics.is_none() => CheckConstraint {
                name: None,
                expr: Box::new(self.constraint_expr(node)?),
                no_inherit: kws.contains(&"inherit"),
                enforced: None,
            }
            .into(),
            ["generated", ..] if characteristics.is_none() => self.generated(node, &kws)?,
            ["primary", "key"] => {
                if node.child(SyntaxKind::IndexParams).is_some() {
                    return unsupported("index parameters");
                }
                PrimaryKeyConstraint {
                    name: None,
                    index_name: None,
                    index_type: None,
                    columns: vec![],
                    include: vec![],
                    index_options: vec![],
                    characteristics,
                }
                .into()
            }
            ["unique"] => {
                if node.child(SyntaxKind::IndexParams).is_some() {
                    return unsupported("index parameters");
                }
                UniqueConstraint {
                    name: None,
                    index_name: None,
                    index_type_display: KeyOrIndexDisplay::None,
                    index_type: None,
                    columns: vec![],
                    include: vec![],
                    index_options: vec![],
                    characteristics,
                    nulls_distinct: NullsDistinctOption::None,
                }
                .into()
            }
            [] if node.child(SyntaxKind::ReferencesClause).is_some() => {
                let Some(refs) = node.child(SyntaxKind::ReferencesClause) else {
                    return unsupported("REFERENCES");
                };
                self.foreign_key(None, vec![], refs, characteristics)?
                    .into()
            }
            _ => return unsupported(format!("column constraint {}", kws.join(" "))),
        };
        Ok(ColumnOptionDef { name, option })
    }

    fn generated(&self, node: &SyntaxNode, kws: &[&str]) -> Result<ColumnOption> {
        if node.child(SyntaxKind::SequenceOptions).is_some() {
            return unsupported("identity sequence options");
        }
        let identity = |generated_as| ColumnOption::Generated {
            generated_as,
            sequence_options: Some(vec![]),
            generation_expr: None,
            generation_expr_mode: None,
            generated_keyword: true,
        };
        Ok(match kws {
            ["generated", "always", "as", "identity"] => identity(GeneratedAs::Always),
            ["generated", "by", "default", "as", "identity"] => identity(GeneratedAs::ByDefault),
            ["generated", "always", "as", "stored"] => ColumnOption::Generated {
                generated_as: GeneratedAs::ExpStored,
                sequence_options: None,
                generation_expr: Some(self.constraint_expr(node)?),
                generation_expr_mode: Some(GeneratedExpressionMode::Stored),
                generated_keyword: true,
            },
            _ => return unsupported(format!("column constraint {}", kws.join(" "))),
        })
    }

    /// `DEFERRABLE`, `INITIALLY ...` after a unique, primary or foreign key, each at most
    /// once (as sqlparser takes them).
    /// Whether the last attribute of a constraint is `NOT VALID` (`ALTER TABLE ... ADD
    /// CONSTRAINT` only). [`Self::characteristics`] leaves it out.
    fn not_valid(&self, node: &SyntaxNode) -> bool {
        node.nodes()
            .filter(|n| n.kind == SyntaxKind::ConstraintAttribute)
            .last()
            .is_some_and(|a| self.keywords(a) == ["not", "valid"])
    }

    fn characteristics(&self, node: &SyntaxNode) -> Result<Option<ConstraintCharacteristics>> {
        let mut cc = ConstraintCharacteristics::default();
        let mut any = false;
        let mut attributes: Vec<_> = node
            .nodes()
            .filter(|n| n.kind == SyntaxKind::ConstraintAttribute)
            .collect();
        if self.not_valid(node) {
            attributes.pop();
        }
        for attribute in attributes {
            any = true;
            let kws = self.keywords(attribute);
            match kws.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                ["deferrable"] if cc.deferrable.is_none() => cc.deferrable = Some(true),
                ["not", "deferrable"] if cc.deferrable.is_none() => cc.deferrable = Some(false),
                ["initially", "deferred"] if cc.initially.is_none() => {
                    cc.initially = Some(DeferrableInitial::Deferred);
                }
                ["initially", "immediate"] if cc.initially.is_none() => {
                    cc.initially = Some(DeferrableInitial::Immediate);
                }
                _ => return unsupported(format!("constraint attribute {}", kws.join(" "))),
            }
        }
        Ok(any.then_some(cc))
    }

    fn foreign_key(
        &self,
        name: Option<Ident>,
        columns: Vec<Ident>,
        refs: &SyntaxNode,
        characteristics: Option<ConstraintCharacteristics>,
    ) -> Result<ForeignKeyConstraint> {
        let Some(table) = refs.child(SyntaxKind::Name) else {
            return unsupported("referenced table");
        };
        let referred_columns = match refs.child(SyntaxKind::ColumnList) {
            Some(list) => self.column_list(list)?,
            None => vec![],
        };
        let mut match_kind = None;
        let mut on_delete = None;
        let mut on_update = None;
        for part in refs.nodes() {
            let kws = self.keywords(part);
            let kws: Vec<&str> = kws.iter().map(String::as_str).collect();
            match part.kind {
                SyntaxKind::MatchClause if match_kind.is_none() => {
                    match_kind = Some(match kws[..] {
                        [_, "full"] => ConstraintReferenceMatchKind::Full,
                        [_, "partial"] => ConstraintReferenceMatchKind::Partial,
                        [_, "simple"] => ConstraintReferenceMatchKind::Simple,
                        _ => return unsupported("MATCH"),
                    });
                }
                SyntaxKind::ReferentialAction => {
                    if part.child(SyntaxKind::ColumnList).is_some() {
                        return unsupported("SET NULL / DEFAULT column list");
                    }
                    let action = match kws[2..] {
                        ["restrict"] => ReferentialAction::Restrict,
                        ["cascade"] => ReferentialAction::Cascade,
                        ["set", "null"] => ReferentialAction::SetNull,
                        ["no", "action"] => ReferentialAction::NoAction,
                        ["set", "default"] => ReferentialAction::SetDefault,
                        _ => return unsupported("referential action"),
                    };
                    let slot = if kws[1] == "delete" {
                        &mut on_delete
                    } else {
                        &mut on_update
                    };
                    if slot.is_some() {
                        return unsupported("repeated referential action");
                    }
                    *slot = Some(action);
                }
                SyntaxKind::Name | SyntaxKind::ColumnList => {}
                _ => return unsupported("REFERENCES"),
            }
        }
        Ok(ForeignKeyConstraint {
            name,
            index_name: None,
            columns,
            foreign_table: object_name(self.name_parts(table)?),
            referred_columns,
            on_delete,
            on_update,
            match_kind,
            characteristics,
        })
    }

    fn index_columns(&self, node: &SyntaxNode) -> Result<Vec<IndexColumn>> {
        let Some(list) = node.child(SyntaxKind::ColumnList) else {
            return unsupported("index columns");
        };
        Ok(self
            .column_list(list)?
            .into_iter()
            .map(|ident| IndexColumn {
                column: OrderByExpr {
                    expr: Expr::Identifier(ident),
                    options: OrderByOptions {
                        sort: None,
                        nulls_first: None,
                    },
                    with_fill: None,
                },
                operator_class: None,
            })
            .collect())
    }

    /// `INCLUDE (cols)`; other index parameters are unsupported.
    fn include(&self, node: &SyntaxNode) -> Result<Vec<Ident>> {
        let mut include = vec![];
        for params in node.nodes().filter(|n| n.kind == SyntaxKind::IndexParams) {
            match (
                self.keywords(params).first(),
                params.child(SyntaxKind::ColumnList),
            ) {
                (Some(kw), Some(list)) if kw == "include" && include.is_empty() => {
                    include = self.column_list(list)?;
                }
                _ => return unsupported("index parameters"),
            }
        }
        Ok(include)
    }

    fn table_constraint(&self, node: &SyntaxNode) -> Result<TableConstraint> {
        let (name, kws) = self.constraint_head(node)?;
        let characteristics = self.characteristics(node)?;
        let kws: Vec<&str> = kws.iter().map(String::as_str).collect();
        Ok(match kws[..] {
            ["check", ..] if characteristics.is_none() => CheckConstraint {
                name,
                expr: Box::new(self.constraint_expr(node)?),
                no_inherit: kws.contains(&"inherit"),
                enforced: None,
            }
            .into(),
            ["primary", "key", ..] if !kws.contains(&"using") => PrimaryKeyConstraint {
                name,
                index_name: None,
                index_type: None,
                columns: self.index_columns(node)?,
                include: self.include(node)?,
                index_options: vec![],
                characteristics,
            }
            .into(),
            ["unique", ..] if !kws.contains(&"using") => UniqueConstraint {
                name,
                index_name: None,
                index_type_display: KeyOrIndexDisplay::None,
                index_type: None,
                columns: self.index_columns(node)?,
                include: self.include(node)?,
                index_options: vec![],
                characteristics,
                nulls_distinct: match kws[1..] {
                    ["nulls", "distinct"] => NullsDistinctOption::Distinct,
                    ["nulls", "not", "distinct"] => NullsDistinctOption::NotDistinct,
                    _ => NullsDistinctOption::None,
                },
            }
            .into(),
            ["foreign", "key"] => {
                let (Some(columns), Some(refs)) = (
                    node.child(SyntaxKind::ColumnList),
                    node.child(SyntaxKind::ReferencesClause),
                ) else {
                    return unsupported("FOREIGN KEY");
                };
                self.foreign_key(name, self.column_list(columns)?, refs, characteristics)?
                    .into()
            }
            _ => return unsupported(format!("table constraint {}", kws.join(" "))),
        })
    }

    fn names(&self, node: &SyntaxNode) -> Result<Vec<sqlparser::ast::ObjectName>> {
        node.nodes()
            .filter(|n| n.kind == SyntaxKind::Name)
            .map(|n| Ok(object_name(self.name_parts(n)?)))
            .collect()
    }

    pub(super) fn create_view(&self, node: &SyntaxNode) -> Result<Statement> {
        let kws = self.keywords(node);
        let has = |kw: &str| kws.iter().any(|k| k == kw);
        if has("recursive") || node.child(SyntaxKind::TableOption).is_some() {
            return unsupported("view option");
        }
        let (Some(name), Some(query)) = (
            node.child(SyntaxKind::Name),
            node.child(SyntaxKind::SelectStmt),
        ) else {
            return unsupported("CREATE VIEW");
        };
        let columns = match node.child(SyntaxKind::ColumnList) {
            Some(list) => self
                .column_list(list)?
                .into_iter()
                .map(|name| ViewColumnDef {
                    name,
                    data_type: None,
                    options: None,
                })
                .collect(),
            None => vec![],
        };
        Ok(Statement::CreateView(CreateView {
            or_alter: false,
            or_replace: has("replace"),
            materialized: has("materialized"),
            secure: false,
            name: object_name(self.name_parts(name)?),
            name_before_not_exists: false,
            columns,
            query: Box::new(self.query(query)?),
            options: CreateTableOptions::None,
            cluster_by: vec![],
            comment: None,
            with_no_schema_binding: false,
            if_not_exists: has("exists"),
            temporary: has("temp") || has("temporary"),
            copy_grants: false,
            to: None,
            params: None,
        }))
    }

    pub(super) fn create_type(&self, node: &SyntaxNode) -> Result<Statement> {
        let Some(name) = node.child(SyntaxKind::Name) else {
            return unsupported("type name");
        };
        let representation = if let Some(labels) = node.child(SyntaxKind::EnumLabels) {
            let labels = labels
                .tokens()
                .filter(|t| t.kind == SyntaxKind::String)
                // sqlparser gives string labels no span.
                .map(|t| sqlparser::ast::Ident {
                    value: unquote(self.text(t), 0),
                    quote_style: Some('\''),
                    span: sqlparser::tokenizer::Span::empty(),
                })
                .collect();
            Some(UserDefinedTypeRepresentation::Enum { labels })
        } else if let Some(attributes) = node.child(SyntaxKind::TableElements) {
            let attributes = attributes
                .nodes()
                .map(|a| {
                    let (Some(name), Some(data_type)) =
                        (a.tokens().next(), a.child(SyntaxKind::TypeName))
                    else {
                        return unsupported("attribute");
                    };
                    Ok(UserDefinedTypeCompositeAttributeDef {
                        name: self.ident(name)?,
                        data_type: self.data_type(data_type)?,
                        collation: a
                            .child(SyntaxKind::Name)
                            .map(|c| Ok(object_name(self.name_parts(c)?)))
                            .transpose()?,
                    })
                })
                .collect::<Result<_>>()?;
            Some(UserDefinedTypeRepresentation::Composite { attributes })
        } else if node.child(SyntaxKind::TableOption).is_some() {
            return unsupported("CREATE TYPE form");
        } else {
            None
        };
        Ok(Statement::CreateType {
            name: object_name(self.name_parts(name)?),
            representation,
        })
    }

    pub(super) fn create_index(&self, node: &SyntaxNode) -> Result<Statement> {
        let kws = self.keywords(node);
        let has = |kw: &str| kws.iter().any(|k| k == kw);
        if has("only") {
            return unsupported("ON ONLY");
        }
        let names = self.names(node)?;
        let (name, table_name) = match <[_; 2]>::try_from(names) {
            Ok([name, table]) => (Some(name), table),
            Err(mut names) if names.len() == 1 => (None, names.remove(0)),
            Err(_) => return unsupported("index names"),
        };
        let mut using = None;
        let mut include = vec![];
        for params in node.nodes().filter(|n| n.kind == SyntaxKind::IndexParams) {
            let words = self.keywords(params);
            match words.first().map(String::as_str) {
                Some("using") if using.is_none() => {
                    let Some(method) = params
                        .child(SyntaxKind::Name)
                        .and_then(|n| n.tokens().next())
                    else {
                        return unsupported("USING");
                    };
                    using = Some(match self.text(method).to_ascii_lowercase().as_str() {
                        _ if method.kind != SyntaxKind::Ident => {
                            IndexType::Custom(self.ident(method)?)
                        }
                        "btree" => IndexType::BTree,
                        "hash" => IndexType::Hash,
                        "gin" => IndexType::GIN,
                        "gist" => IndexType::GiST,
                        "spgist" => IndexType::SPGiST,
                        "brin" => IndexType::BRIN,
                        "bloom" => IndexType::Bloom,
                        _ => IndexType::Custom(self.ident(method)?),
                    });
                }
                Some("include") if include.is_empty() => {
                    let Some(list) = params.child(SyntaxKind::ColumnList) else {
                        return unsupported("INCLUDE");
                    };
                    include = self.column_list(list)?;
                }
                _ => return unsupported("index parameters"),
            }
        }
        let Some(elems) = node.child(SyntaxKind::IndexElems) else {
            return unsupported("index columns");
        };
        let exprs = self.order_by_exprs(elems)?;
        let columns = exprs
            .into_iter()
            .zip(elems.nodes())
            .map(|(column, elem)| {
                Ok(IndexColumn {
                    column,
                    operator_class: elem
                        .nodes()
                        .nth(1)
                        .filter(|n| n.kind == SyntaxKind::Name)
                        .map(|n| Ok(object_name(self.name_parts(n)?)))
                        .transpose()?,
                })
            })
            .collect::<Result<_>>()?;
        let nulls_distinct = if has("nulls") {
            Some(!has("not"))
        } else {
            None
        };
        let predicate = match node.child(SyntaxKind::WhereClause) {
            Some(w) => Some(self.constraint_expr(w)?),
            None => None,
        };
        Ok(Statement::CreateIndex(CreateIndex {
            name,
            table_name,
            using,
            columns,
            unique: has("unique"),
            concurrently: has("concurrently"),
            r#async: false,
            if_not_exists: has("exists"),
            include,
            nulls_distinct,
            with: vec![],
            predicate,
            index_options: vec![],
            alter_options: vec![],
        }))
    }

    pub(super) fn drop(&self, node: &SyntaxNode) -> Result<Statement> {
        let kws = self.keywords(node);
        let has = |kw: &str| kws.iter().any(|k| k == kw);
        let object_type = match kws.get(1).map(String::as_str) {
            Some("table") => ObjectType::Table,
            Some("view") => ObjectType::View,
            Some("materialized") => ObjectType::MaterializedView,
            Some("index") => ObjectType::Index,
            Some("type") => ObjectType::Type,
            Some("sequence") => ObjectType::Sequence,
            Some("schema") => ObjectType::Schema,
            _ => return unsupported(format!("DROP {}", kws.get(1).map_or("", String::as_str))),
        };
        if has("concurrently") {
            return unsupported("DROP INDEX CONCURRENTLY");
        }
        Ok(Statement::Drop {
            object_type,
            if_exists: has("exists"),
            names: self.names(node)?,
            cascade: has("cascade"),
            restrict: has("restrict"),
            purge: false,
            temporary: false,
            table: None,
        })
    }

    /// The single-token names (`Name` nodes) directly in a node, as identifiers.
    fn simple_names(&self, node: &SyntaxNode) -> Result<Vec<Ident>> {
        node.nodes()
            .filter(|n| n.kind == SyntaxKind::Name)
            .map(|n| match n.tokens().next() {
                Some(t) => self.ident(t),
                None => unsupported("name"),
            })
            .collect()
    }

    pub(super) fn alter_table(&self, node: &SyntaxNode) -> Result<Statement> {
        let kws = self.keywords(node);
        if node.tokens().any(|t| t.kind == SyntaxKind::Operator) {
            return unsupported("ALTER TABLE name *");
        }
        let Some(name) = node.child(SyntaxKind::Name) else {
            return unsupported("table name");
        };
        let operations = node
            .nodes()
            .filter(|n| n.kind == SyntaxKind::AlterTableAction)
            .map(|a| self.alter_table_action(a))
            .collect::<Result<_>>()?;
        // sqlparser keeps the `;` after the statement, or else its last token.
        let end = node.range.end as usize;
        let rest = self.source[end..].trim_start_matches([' ', '\t', '\r', '\n']);
        let end_token = if rest.starts_with(';') {
            let at = (self.source.len() - rest.len()) as u32;
            sqlparser::tokenizer::TokenWithSpan::new(
                sqlparser::tokenizer::Token::SemiColon,
                self.span(crate::TextRange::new(at, at + 1)),
            )
        } else {
            let Some(last) = super::all_tokens(node).last().copied() else {
                return unsupported("ALTER TABLE");
            };
            match self.sqlparser_tokens(last.range)?.pop() {
                Some(token) => token,
                None => return unsupported("last token"),
            }
        };
        Ok(Statement::AlterTable(AlterTable {
            name: object_name(self.name_parts(name)?),
            if_exists: kws.get(2).is_some_and(|k| k == "if"),
            only: kws.iter().any(|k| k == "only"),
            operations,
            location: None,
            on_cluster: None,
            table_type: None,
            end_token: sqlparser::ast::helpers::attached_token::AttachedToken(end_token),
        }))
    }

    fn alter_table_action(&self, node: &SyntaxNode) -> Result<AlterTableOperation> {
        let kws = self.keywords(node);
        let kws: Vec<&str> = kws.iter().map(String::as_str).collect();
        let behavior = || match kws.last() {
            Some(&"cascade") => Some(DropBehavior::Cascade),
            Some(&"restrict") => Some(DropBehavior::Restrict),
            _ => None,
        };
        let names = self.simple_names(node)?;
        let one_name = || match &names[..] {
            [name] => Ok(name.clone()),
            _ => unsupported("name"),
        };
        if let Some(option) = node.child(SyntaxKind::TableOption) {
            let words = self.keywords(option);
            return Ok(
                match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    ["enable", "row", "level", "security"] => {
                        AlterTableOperation::EnableRowLevelSecurity {}
                    }
                    ["disable", "row", "level", "security"] => {
                        AlterTableOperation::DisableRowLevelSecurity {}
                    }
                    ["force", "row", "level", "security"] => {
                        AlterTableOperation::ForceRowLevelSecurity
                    }
                    ["no", "force", "row", "level", "security"] => {
                        AlterTableOperation::NoForceRowLevelSecurity
                    }
                    ["set", "logged"] => AlterTableOperation::SetLogged,
                    ["set", "unlogged"] => AlterTableOperation::SetUnlogged,
                    _ => {
                        return unsupported(format!(
                            "ALTER TABLE {}",
                            words.first().map_or("", String::as_str)
                        ))
                    }
                },
            );
        }
        Ok(match kws[..] {
            ["add", ..] if node.child(SyntaxKind::TableConstraint).is_some() => {
                let Some(constraint) = node.child(SyntaxKind::TableConstraint) else {
                    return unsupported("constraint");
                };
                AlterTableOperation::AddConstraint {
                    constraint: self.table_constraint(constraint)?,
                    not_valid: self.not_valid(constraint),
                }
            }
            ["add", ..] => {
                let Some(column) = node.child(SyntaxKind::ColumnDef) else {
                    return unsupported("column");
                };
                AlterTableOperation::AddColumn {
                    column_keyword: kws.get(1) == Some(&"column"),
                    if_not_exists: kws.contains(&"exists"),
                    column_def: self.column_def(column)?,
                    column_position: None,
                }
            }
            ["drop", "constraint", ..] => AlterTableOperation::DropConstraint {
                if_exists: kws.contains(&"exists"),
                name: one_name()?,
                drop_behavior: behavior(),
            },
            ["drop", ..] => AlterTableOperation::DropColumn {
                has_column_keyword: kws.get(1) == Some(&"column"),
                column_names: vec![one_name()?],
                if_exists: kws.get(1..3) == Some(&["if", "exists"][..])
                    || kws.get(2..4) == Some(&["if", "exists"][..]),
                drop_behavior: behavior(),
            },
            ["alter", ..] => {
                let Some(column_name) = names.first().cloned() else {
                    return unsupported("column name");
                };
                AlterTableOperation::AlterColumn {
                    column_name,
                    op: self.alter_column(node, &kws)?,
                }
            }
            ["rename", "to"] if names.len() == 1 => AlterTableOperation::RenameTable {
                table_name: RenameTableNameKind::To(object_name(vec![one_name()?])),
            },
            ["rename", "constraint" | "column", "to"] | ["rename", "to"] if names.len() == 2 => {
                let (old, new) = (names[0].clone(), names[1].clone());
                if kws[1] == "constraint" {
                    AlterTableOperation::RenameConstraint {
                        old_name: old,
                        new_name: new,
                    }
                } else {
                    AlterTableOperation::RenameColumn {
                        old_column_name: old,
                        new_column_name: new,
                    }
                }
            }
            ["owner", "to", ..] => {
                let owner = one_name()?;
                let new_owner = if owner.quote_style.is_some() {
                    Owner::Ident(owner)
                } else {
                    match owner.value.to_ascii_lowercase().as_str() {
                        "current_user" => Owner::CurrentUser,
                        "current_role" => Owner::CurrentRole,
                        "session_user" => Owner::SessionUser,
                        _ => Owner::Ident(owner),
                    }
                };
                AlterTableOperation::OwnerTo { new_owner }
            }
            ["validate", "constraint", ..] => {
                AlterTableOperation::ValidateConstraint { name: one_name()? }
            }
            _ => return unsupported(format!("ALTER TABLE {}", kws.join(" "))),
        })
    }

    fn alter_column(&self, node: &SyntaxNode, kws: &[&str]) -> Result<AlterColumnOperation> {
        let column_kw = usize::from(kws.get(1) == Some(&"column"));
        let rest = &kws[1 + column_kw..];
        Ok(match rest {
            ["set", "not", "null"] => AlterColumnOperation::SetNotNull,
            ["drop", "not", "null"] => AlterColumnOperation::DropNotNull,
            ["set", "default"] => AlterColumnOperation::SetDefault {
                value: self.constraint_expr(node)?,
            },
            ["drop", "default"] => AlterColumnOperation::DropDefault,
            ["type"] | ["set", "data", "type"] => {
                if node.child(SyntaxKind::ColumnConstraint).is_some() {
                    return unsupported("TYPE ... COLLATE");
                }
                let Some(data_type) = node.child(SyntaxKind::TypeName) else {
                    return unsupported("type");
                };
                let using = match node.child(SyntaxKind::UsingClause) {
                    Some(u) => Some(self.constraint_expr(u)?),
                    None => None,
                };
                AlterColumnOperation::SetDataType {
                    data_type: self.data_type(data_type)?,
                    using,
                    had_set: rest[0] == "set",
                }
            }
            ["add", "generated", ..] => {
                if node.child(SyntaxKind::SequenceOptions).is_some() {
                    return unsupported("identity sequence options");
                }
                AlterColumnOperation::AddGenerated {
                    generated_as: Some(if rest.contains(&"always") {
                        GeneratedAs::Always
                    } else {
                        GeneratedAs::ByDefault
                    }),
                    sequence_options: None,
                }
            }
            _ => return unsupported(format!("ALTER COLUMN {}", rest.join(" "))),
        })
    }
}
