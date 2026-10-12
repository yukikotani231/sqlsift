//! Lowering of schema statements (`CREATE TABLE`, ...).

use sqlparser::ast::helpers::stmt_create_table::CreateTableBuilder;
use sqlparser::ast::{
    CheckConstraint, ColumnDef, ColumnOption, ColumnOptionDef, ConstraintCharacteristics,
    ConstraintReferenceMatchKind, DeferrableInitial, Expr, ForeignKeyConstraint, GeneratedAs,
    GeneratedExpressionMode, Ident, IndexColumn, KeyOrIndexDisplay, NullsDistinctOption, OnCommit,
    OrderByExpr, OrderByOptions, PrimaryKeyConstraint, ReferentialAction, Statement,
    TableConstraint, UniqueConstraint, WithData,
};

use super::{object_name, unsupported, Lowering, Result};
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
                    SyntaxKind::TableConstraint => {
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
    fn characteristics(&self, node: &SyntaxNode) -> Result<Option<ConstraintCharacteristics>> {
        let mut cc = ConstraintCharacteristics::default();
        let mut any = false;
        for attribute in node
            .nodes()
            .filter(|n| n.kind == SyntaxKind::ConstraintAttribute)
        {
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
}
