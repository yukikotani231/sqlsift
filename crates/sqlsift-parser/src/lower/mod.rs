//! Lowering: turns statements of the syntax tree into sqlparser's AST, so the analyzer can
//! run on this parser's output unchanged while the parser is being built.
//!
//! This is a temporary bridge (see #257): the analyzer is meant to read the new tree
//! directly later. A construct the lowering doesn't handle yet gives
//! [`Unsupported`], never a guess, so a statement either lowers to exactly what sqlparser
//! would produce or not at all. The differential test checks that.

// `|p| p.parse_value()` can't be a method path here: the path would fix the parser's
// lifetime, and `with_sqlparser` creates the parser itself.
#![allow(clippy::redundant_closure_for_method_calls)]

use sqlparser::ast::{
    self, BinaryOperator, CaseWhen, CastKind, Cte, CteAsMaterialized, Distinct, Expr, Function,
    FunctionArg, FunctionArgExpr, FunctionArgumentList, FunctionArguments, GroupByExpr, Ident,
    Join, JoinConstraint, JoinOperator, LimitClause, ObjectName, ObjectNamePart, Offset,
    OffsetRows, OrderBy, OrderByExpr, OrderByKind, OrderByOptions, Query, Select, SelectFlavor,
    SelectItem, SelectItemQualifiedWildcardKind, SetExpr, SetOperator, SetQuantifier, Statement,
    TableAlias, TableAliasColumnDef, TableFactor, TableWithJoins, UnaryOperator, Value,
    ValueWithSpan, WildcardAdditionalOptions, With,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::ParserError;
use sqlparser::tokenizer::{Location, Span, Token, TokenWithSpan};

use crate::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

mod ddl;

/// Function names sqlparser parses as special forms (`TRIM(x FROM y)`, `CEIL(x TO DAY)`,
/// geometric literals, ...) rather than as function calls.
const SPECIAL_FUNCTIONS: &[&str] = &[
    "array",
    "box",
    "ceil",
    "circle",
    "convert",
    "extract",
    "floor",
    "interval",
    "lambda",
    "line",
    "lseg",
    "map",
    "match",
    "overlay",
    "path",
    "point",
    "polygon",
    "position",
    "safe_cast",
    "struct",
    "substr",
    "substring",
    "trim",
    "try_cast",
    "try_convert",
];

/// Calls that mean something else at the top of GROUP BY.
const GROUPING_FUNCTIONS: &[&str] = &["cube", "rollup", "grouping"];

/// A construct the lowering doesn't handle (yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported(pub String);

type Result<T> = std::result::Result<T, Unsupported>;

fn unsupported<T>(what: impl Into<String>) -> Result<T> {
    Err(Unsupported(what.into()))
}

/// Byte offset → sqlparser [`Location`] (1-based line and column, columns in characters).
pub struct LineIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { text, line_starts }
    }

    pub fn location(&self, offset: u32) -> Location {
        let offset = offset as usize;
        let line = self.line_starts.partition_point(|&s| s <= offset) - 1;
        let column = self.text[self.line_starts[line]..offset].chars().count() + 1;
        Location::new(line as u64 + 1, column as u64)
    }

    pub fn span(&self, range: TextRange) -> Span {
        Span::new(self.location(range.start), self.location(range.end))
    }
}

/// Lowers one statement node (as returned by [`crate::Parse::statements`]).
///
/// `source` is the text the tree was parsed from. Spans are relative to it.
pub fn lower_statement(source: &str, node: &SyntaxNode) -> Result<Statement> {
    let cx = Lowering {
        source,
        lines: LineIndex::new(source),
    };
    if node.has_errors() {
        return unsupported("syntax error");
    }
    // sqlparser reads `/*+ ... */` (and `--+ ...`) comments as optimizer hints, even for
    // PostgreSQL, where they are comments.
    if node.descendant_tokens().iter().any(|t| {
        matches!(t.kind, SyntaxKind::BlockComment | SyntaxKind::LineComment)
            && matches!(t.text(source).get(..3), Some("/*+" | "--+"))
    }) {
        return unsupported("optimizer hint comment");
    }
    match node.kind {
        SyntaxKind::SelectStmt => Ok(Statement::Query(Box::new(cx.query(node)?))),
        SyntaxKind::InsertStmt | SyntaxKind::UpdateStmt | SyntaxKind::DeleteStmt => cx.dml(node),
        SyntaxKind::CreateTableStmt => cx.create_table(node),
        SyntaxKind::CreateViewStmt => cx.create_view(node),
        SyntaxKind::CreateTypeStmt => cx.create_type(node),
        SyntaxKind::CreateIndexStmt => cx.create_index(node),
        SyntaxKind::DropStmt => cx.drop(node),
        SyntaxKind::AlterTableStmt => cx.alter_table(node),
        SyntaxKind::OpaqueStmt => unsupported("statement"),
        kind => unsupported(format!("{kind:?}")),
    }
}

struct Lowering<'a> {
    source: &'a str,
    lines: LineIndex<'a>,
}

/// The non-trivia tokens of a node, children's included.
fn all_tokens<'a>(node: &'a SyntaxNode<'a>) -> Vec<&'a SyntaxToken> {
    node.descendant_tokens()
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .collect()
}

impl Lowering<'_> {
    fn text(&self, token: &SyntaxToken) -> &str {
        token.text(self.source)
    }

    fn is_kw(&self, token: &SyntaxToken, kw: &str) -> bool {
        token.kind == SyntaxKind::Ident && self.text(token).eq_ignore_ascii_case(kw)
    }

    fn has_kw(&self, node: &SyntaxNode, kw: &str) -> bool {
        node.tokens().any(|t| self.is_kw(t, kw))
    }

    fn span(&self, range: TextRange) -> Span {
        self.lines.span(range)
    }

    fn attached(&self, token: &SyntaxToken) -> ast::helpers::attached_token::AttachedToken {
        let word = Token::make_word(self.text(token), None);
        ast::helpers::attached_token::AttachedToken(TokenWithSpan::new(
            word,
            self.span(token.range),
        ))
    }

    fn ident(&self, token: &SyntaxToken) -> Result<Ident> {
        let text = self.text(token);
        let span = self.span(token.range);
        match token.kind {
            SyntaxKind::Ident => Ok(Ident::with_span(span, text)),
            SyntaxKind::QuotedIdent if text.starts_with('"') => {
                let inner = &text[1..text.len().saturating_sub(1).max(1)];
                Ok(Ident {
                    value: inner.replace("\"\"", "\""),
                    quote_style: Some('"'),
                    span,
                })
            }
            _ => unsupported("identifier"),
        }
    }

    // --- queries ---

    fn query(&self, node: &SyntaxNode) -> Result<Query> {
        let mut order_by = None;
        let mut limit = None;
        let mut offset = None;
        let mut locks = vec![];
        for child in node.nodes() {
            match child.kind {
                SyntaxKind::LockingClause => locks.push(self.lock(child)?),
                SyntaxKind::OrderByClause => order_by = Some(self.order_by(child)?),
                SyntaxKind::LimitClause => {
                    // `LIMIT ALL` is no limit.
                    if !child.tokens().any(|t| self.is_kw(t, "all")) {
                        limit = Some(self.expr(expr_child(child)?)?);
                    }
                }
                SyntaxKind::OffsetClause => {
                    let rows = if self.has_kw(child, "rows") {
                        OffsetRows::Rows
                    } else if self.has_kw(child, "row") {
                        OffsetRows::Row
                    } else {
                        OffsetRows::None
                    };
                    offset = Some(Offset {
                        value: self.expr(expr_child(child)?)?,
                        rows,
                    });
                }
                _ => {}
            }
        }
        let limit_clause =
            (limit.is_some() || offset.is_some()).then(|| LimitClause::LimitOffset {
                limit,
                offset,
                limit_by: vec![],
            });
        let with = node
            .child(SyntaxKind::WithClause)
            .map(|w| self.with(w))
            .transpose()?;
        let body = node
            .nodes()
            .find(|n| {
                matches!(
                    n.kind,
                    SyntaxKind::SimpleSelect
                        | SyntaxKind::SetOperation
                        | SyntaxKind::ParenSelect
                        | SyntaxKind::ValuesClause
                        | SyntaxKind::InsertStmt
                        | SyntaxKind::UpdateStmt
                        | SyntaxKind::DeleteStmt
                )
            })
            .ok_or_else(|| Unsupported("query body".into()))?;
        Ok(Query {
            with,
            body: Box::new(self.set_expr(body)?),
            order_by,
            limit_clause,
            fetch: None,
            locks,
            for_clause: None,
            settings: None,
            format_clause: None,
            pipe_operators: vec![],
        })
    }

    fn lock(&self, node: &SyntaxNode) -> Result<ast::LockClause> {
        if self.has_kw(node, "key") {
            return unsupported("FOR NO KEY UPDATE / FOR KEY SHARE");
        }
        let names: Vec<_> = node.nodes().collect();
        let of = match names[..] {
            [] => None,
            [name] => Some(object_name(self.name_parts(name)?)),
            _ => return unsupported("FOR ... OF several tables"),
        };
        let nonblock = if self.has_kw(node, "nowait") {
            Some(ast::NonBlock::Nowait)
        } else if self.has_kw(node, "skip") {
            Some(ast::NonBlock::SkipLocked)
        } else {
            None
        };
        Ok(ast::LockClause {
            lock_type: if self.has_kw(node, "update") {
                ast::LockType::Update
            } else {
                ast::LockType::Share
            },
            of,
            nonblock,
        })
    }

    fn set_expr(&self, node: &SyntaxNode) -> Result<SetExpr> {
        match node.kind {
            SyntaxKind::SimpleSelect => Ok(SetExpr::Select(Box::new(self.select(node)?))),
            SyntaxKind::ParenSelect => {
                Ok(SetExpr::Query(Box::new(self.query(child_query(node)?)?)))
            }
            SyntaxKind::SetOperation => {
                let parts: Vec<_> = node.nodes().collect();
                let [left, right] = parts[..] else {
                    return unsupported("set operation");
                };
                let tokens: Vec<_> = node.tokens().collect();
                let op = if self.is_kw(tokens[0], "union") {
                    SetOperator::Union
                } else if self.is_kw(tokens[0], "intersect") {
                    SetOperator::Intersect
                } else {
                    SetOperator::Except
                };
                let set_quantifier = match tokens.get(1) {
                    Some(t) if self.is_kw(t, "all") => SetQuantifier::All,
                    Some(t) if self.is_kw(t, "distinct") => SetQuantifier::Distinct,
                    _ => SetQuantifier::None,
                };
                Ok(SetExpr::SetOperation {
                    left: Box::new(self.set_expr(left)?),
                    op,
                    set_quantifier,
                    right: Box::new(self.set_expr(right)?),
                })
            }
            SyntaxKind::InsertStmt => Ok(SetExpr::Insert(self.dml(node)?)),
            SyntaxKind::UpdateStmt => Ok(SetExpr::Update(self.dml(node)?)),
            SyntaxKind::DeleteStmt => Ok(SetExpr::Delete(self.dml(node)?)),
            SyntaxKind::ValuesClause => {
                let mut rows = vec![];
                for row in node.nodes() {
                    let tokens: Vec<_> = row.tokens().collect();
                    let (Some(open), Some(close)) = (tokens.first(), tokens.last()) else {
                        return unsupported("VALUES");
                    };
                    rows.push(ast::Parens {
                        opening_token: ast::helpers::attached_token::AttachedToken(
                            TokenWithSpan::new(Token::LParen, self.span(open.range)),
                        ),
                        content: self.expr_list(row)?,
                        closing_token: ast::helpers::attached_token::AttachedToken(
                            TokenWithSpan::new(Token::RParen, self.span(close.range)),
                        ),
                    });
                }
                Ok(SetExpr::Values(ast::Values {
                    explicit_row: false,
                    value_keyword: false,
                    rows,
                }))
            }
            kind => unsupported(format!("{kind:?} as a query body")),
        }
    }

    fn with(&self, node: &SyntaxNode) -> Result<With> {
        let tokens: Vec<_> = node.tokens().collect();
        let mut cte_tables = vec![];
        for cte in node.nodes() {
            let name = cte
                .tokens()
                .next()
                .ok_or_else(|| Unsupported("CTE name".into()))?;
            let columns = match cte.child(SyntaxKind::ColumnList) {
                Some(list) => self
                    .column_list(list)?
                    .into_iter()
                    .map(|name| TableAliasColumnDef {
                        name,
                        data_type: None,
                    })
                    .collect(),
                None => vec![],
            };
            let materialized = if self.has_kw(cte, "not") {
                Some(CteAsMaterialized::NotMaterialized)
            } else if self.has_kw(cte, "materialized") {
                Some(CteAsMaterialized::Materialized)
            } else {
                None
            };
            let closing = cte
                .tokens()
                .last()
                .ok_or_else(|| Unsupported("CTE".into()))?;
            cte_tables.push(Cte {
                alias: TableAlias {
                    explicit: false,
                    name: self.ident(name)?,
                    columns,
                    at: None,
                },
                query: Box::new(self.cte_query(cte)?),
                from: None,
                materialized,
                closing_paren_token: ast::helpers::attached_token::AttachedToken(
                    TokenWithSpan::new(Token::RParen, self.span(closing.range)),
                ),
            });
        }
        Ok(With {
            with_token: self.attached(tokens[0]),
            recursive: tokens.get(1).is_some_and(|t| self.is_kw(t, "recursive")),
            cte_tables,
        })
    }

    /// A CTE's query: a `SelectStmt`, or a data-modifying statement wrapped in a query.
    fn cte_query(&self, cte: &SyntaxNode) -> Result<Query> {
        if let Some(query) = cte.child(SyntaxKind::SelectStmt) {
            return self.query(query);
        }
        let dml = cte
            .nodes()
            .find(|n| {
                matches!(
                    n.kind,
                    SyntaxKind::InsertStmt | SyntaxKind::UpdateStmt | SyntaxKind::DeleteStmt
                )
            })
            .ok_or_else(|| Unsupported("CTE".into()))?;
        Ok(Query {
            with: None,
            body: Box::new(self.set_expr(dml)?),
            order_by: None,
            limit_clause: None,
            fetch: None,
            locks: vec![],
            for_clause: None,
            settings: None,
            format_clause: None,
            pipe_operators: vec![],
        })
    }

    // --- INSERT / UPDATE / DELETE ---

    fn dml(&self, node: &SyntaxNode) -> Result<Statement> {
        let tokens: Vec<_> = node.tokens().collect();
        if tokens.iter().any(|t| self.is_kw(t, "only")) {
            return unsupported("ONLY");
        }
        let name = node
            .child(SyntaxKind::Name)
            .ok_or_else(|| Unsupported("table name".into()))?;
        let table_name = object_name(self.name_parts(name)?);
        let alias = node
            .child(SyntaxKind::Alias)
            .map(|a| self.table_alias(a))
            .transpose()?;
        let selection = node
            .child(SyntaxKind::WhereClause)
            .map(|w| {
                if self.has_kw(w, "current") && self.has_kw(w, "of") {
                    unsupported("WHERE CURRENT OF")
                } else {
                    self.expr(expr_child(w)?)
                }
            })
            .transpose()?;
        let returning = node
            .child(SyntaxKind::ReturningClause)
            .map(|r| {
                r.child(SyntaxKind::TargetList)
                    .ok_or_else(|| Unsupported("RETURNING".into()))?
                    .nodes()
                    .map(|item| self.select_item(item))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        match node.kind {
            SyntaxKind::InsertStmt => {
                if node.child(SyntaxKind::OverridingClause).is_some() {
                    return unsupported("OVERRIDING");
                }
                let columns = match node.child(SyntaxKind::ColumnList) {
                    Some(list) => self
                        .column_list(list)?
                        .into_iter()
                        .map(|i| object_name(vec![i]))
                        .collect(),
                    None => vec![],
                };
                let source = node
                    .child(SyntaxKind::SelectStmt)
                    .map(|q| self.query(q).map(Box::new))
                    .transpose()?;
                let on = node
                    .child(SyntaxKind::OnConflictClause)
                    .map(|c| self.on_conflict(c))
                    .transpose()?;
                Ok(Statement::Insert(ast::Insert {
                    insert_token: self.attached(tokens[0]),
                    optimizer_hints: vec![],
                    or: None,
                    ignore: false,
                    into: true,
                    table: ast::TableObject::TableName(table_name),
                    table_alias: alias.map(|a| ast::TableAliasWithoutColumns {
                        explicit: a.explicit,
                        alias: a.name,
                    }),
                    columns,
                    overwrite: false,
                    source,
                    assignments: vec![],
                    partitioned: None,
                    after_columns: vec![],
                    has_table_keyword: false,
                    on,
                    returning,
                    output: None,
                    replace_into: false,
                    priority: None,
                    insert_alias: None,
                    settings: None,
                    format_clause: None,
                    multi_table_insert_type: None,
                    multi_table_into_clauses: vec![],
                    multi_table_when_clauses: vec![],
                    multi_table_else_clause: None,
                }))
            }
            SyntaxKind::UpdateStmt => {
                let from = node
                    .child(SyntaxKind::FromClause)
                    .map(|f| {
                        f.nodes()
                            .map(|item| self.table_with_joins(item))
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?
                    .map(ast::UpdateTableFromKind::AfterSet);
                Ok(Statement::Update(ast::Update {
                    update_token: self.attached(tokens[0]),
                    optimizer_hints: vec![],
                    table: TableWithJoins {
                        relation: plain_table(table_name, alias),
                        joins: vec![],
                    },
                    assignments: self.assignments(
                        node.child(SyntaxKind::SetClause)
                            .ok_or_else(|| Unsupported("SET".into()))?,
                    )?,
                    from,
                    selection,
                    returning,
                    output: None,
                    or: None,
                    order_by: vec![],
                    limit: None,
                }))
            }
            _ => {
                let using = node
                    .child(SyntaxKind::UsingClause)
                    .map(|u| {
                        u.nodes()
                            .map(|item| self.table_with_joins(item))
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?;
                Ok(Statement::Delete(ast::Delete {
                    delete_token: self.attached(tokens[0]),
                    optimizer_hints: vec![],
                    tables: vec![],
                    from: ast::FromTable::WithFromKeyword(vec![TableWithJoins {
                        relation: plain_table(table_name, alias),
                        joins: vec![],
                    }]),
                    using,
                    selection,
                    returning,
                    output: None,
                    order_by: vec![],
                    limit: None,
                }))
            }
        }
    }

    fn assignments(&self, set: &SyntaxNode) -> Result<Vec<ast::Assignment>> {
        set.nodes()
            .map(|a| {
                let mut parts = a.nodes();
                let target = parts
                    .next()
                    .ok_or_else(|| Unsupported("SET target".into()))?;
                let value = parts
                    .next()
                    .ok_or_else(|| Unsupported("SET value".into()))?;
                let target = if target.kind == SyntaxKind::ColumnList {
                    ast::AssignmentTarget::Tuple(
                        self.column_list(target)?
                            .into_iter()
                            .map(|i| object_name(vec![i]))
                            .collect(),
                    )
                } else {
                    ast::AssignmentTarget::ColumnName(object_name(self.name_parts(target)?))
                };
                Ok(ast::Assignment {
                    target,
                    value: self.expr(value)?,
                })
            })
            .collect()
    }

    fn on_conflict(&self, node: &SyntaxNode) -> Result<ast::OnInsert> {
        let conflict_target = match node.child(SyntaxKind::ConflictTarget) {
            None => None,
            Some(t) if self.has_kw(t, "constraint") => {
                Some(ast::ConflictTarget::OnConstraint(object_name(
                    self.name_parts(
                        t.child(SyntaxKind::Name)
                            .ok_or_else(|| Unsupported("constraint".into()))?,
                    )?,
                )))
            }
            Some(t) => {
                if t.child(SyntaxKind::WhereClause).is_some() {
                    return unsupported("ON CONFLICT (...) WHERE");
                }
                let mut columns = vec![];
                for e in t.nodes() {
                    let Expr::Identifier(ident) = self.expr(e)? else {
                        return unsupported("ON CONFLICT on an expression");
                    };
                    columns.push(ident);
                }
                Some(ast::ConflictTarget::Columns(columns))
            }
        };
        let action_node = node
            .child(SyntaxKind::ConflictAction)
            .ok_or_else(|| Unsupported("ON CONFLICT".into()))?;
        let action = if self.has_kw(action_node, "nothing") {
            ast::OnConflictAction::DoNothing
        } else {
            ast::OnConflictAction::DoUpdate(ast::DoUpdate {
                assignments: self.assignments(
                    action_node
                        .child(SyntaxKind::SetClause)
                        .ok_or_else(|| Unsupported("SET".into()))?,
                )?,
                selection: action_node
                    .child(SyntaxKind::WhereClause)
                    .map(|w| self.expr(expr_child(w)?))
                    .transpose()?,
            })
        };
        Ok(ast::OnInsert::OnConflict(ast::OnConflict {
            conflict_target,
            action,
        }))
    }

    fn select(&self, node: &SyntaxNode) -> Result<Select> {
        let clause = node
            .child(SyntaxKind::SelectClause)
            .ok_or_else(|| Unsupported("missing SELECT".into()))?;
        let select_token = clause
            .tokens()
            .next()
            .ok_or_else(|| Unsupported("missing SELECT".into()))?;
        let distinct = if self.has_kw(clause, "distinct") {
            match clause.child(SyntaxKind::ExprList) {
                Some(list) => Some(Distinct::On(self.expr_list(list)?)),
                None => Some(Distinct::Distinct),
            }
        } else if self.has_kw(clause, "all") {
            Some(Distinct::All)
        } else {
            None
        };
        let mut projection = vec![];
        if let Some(list) = clause.child(SyntaxKind::TargetList) {
            for item in list.nodes() {
                projection.push(self.select_item(item)?);
            }
        }
        let mut from = vec![];
        let mut selection = None;
        let mut group_by = GroupByExpr::Expressions(vec![], vec![]);
        let mut having = None;
        let mut named_window = vec![];
        for child in node.nodes() {
            match child.kind {
                SyntaxKind::FromClause => {
                    for item in child.nodes() {
                        from.push(self.table_with_joins(item)?);
                    }
                }
                SyntaxKind::WhereClause => selection = Some(self.expr(expr_child(child)?)?),
                SyntaxKind::GroupByClause => {
                    let list = child
                        .child(SyntaxKind::ExprList)
                        .ok_or_else(|| Unsupported("GROUP BY".into()))?;
                    if list.nodes().any(|n| self.is_call_to(n, GROUPING_FUNCTIONS)) {
                        return unsupported("ROLLUP / CUBE / GROUPING SETS");
                    }
                    group_by = GroupByExpr::Expressions(self.expr_list(list)?, vec![]);
                }
                SyntaxKind::HavingClause => having = Some(self.expr(expr_child(child)?)?),
                SyntaxKind::WindowClause => {
                    for def in child.nodes() {
                        let name = def
                            .tokens()
                            .next()
                            .ok_or_else(|| Unsupported("WINDOW".into()))?;
                        let spec = def
                            .child(SyntaxKind::WindowSpec)
                            .ok_or_else(|| Unsupported("WINDOW".into()))?;
                        named_window.push(ast::NamedWindowDefinition(
                            self.ident(name)?,
                            ast::NamedWindowExpr::WindowSpec(self.window_spec(spec)?),
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(Select {
            select_token: self.attached(select_token),
            optimizer_hints: vec![],
            distinct,
            select_modifiers: None,
            top: None,
            top_before_distinct: false,
            projection,
            exclude: None,
            into: None,
            from,
            lateral_views: vec![],
            prewhere: None,
            selection,
            connect_by: vec![],
            group_by,
            cluster_by: vec![],
            distribute_by: vec![],
            sort_by: vec![],
            having,
            window_before_qualify: !named_window.is_empty(),
            named_window,
            qualify: None,
            value_table_mode: None,
            flavor: SelectFlavor::Standard,
        })
    }

    fn wildcard_options(&self, star: &SyntaxToken) -> WildcardAdditionalOptions {
        WildcardAdditionalOptions {
            wildcard_token: ast::helpers::attached_token::AttachedToken(TokenWithSpan::new(
                Token::Mul,
                self.span(star.range),
            )),
            ..Default::default()
        }
    }

    fn select_item(&self, item: &SyntaxNode) -> Result<SelectItem> {
        if let Some(star) = item.child(SyntaxKind::Star) {
            let token = star
                .tokens()
                .next()
                .ok_or_else(|| Unsupported("*".into()))?;
            return Ok(SelectItem::Wildcard(self.wildcard_options(token)));
        }
        let expr_node = expr_child(item)?;
        // `t.*`
        if expr_node.kind == SyntaxKind::ColumnRef {
            let name = expr_node
                .child(SyntaxKind::Name)
                .ok_or_else(|| Unsupported("name".into()))?;
            if let Some(star) = name.tokens().find(|t| t.kind == SyntaxKind::Operator) {
                let parts = self.name_parts(name)?;
                return Ok(SelectItem::QualifiedWildcard(
                    SelectItemQualifiedWildcardKind::ObjectName(object_name(parts)),
                    self.wildcard_options(star),
                ));
            }
        }
        let expr = self.expr(expr_node)?;
        match item.child(SyntaxKind::Alias) {
            Some(alias) => Ok(SelectItem::ExprWithAlias {
                expr,
                alias: self.alias_ident(alias)?,
            }),
            None => Ok(SelectItem::UnnamedExpr(expr)),
        }
    }

    fn alias_ident(&self, alias: &SyntaxNode) -> Result<Ident> {
        let token = alias
            .tokens()
            .find(|t| !self.is_kw(t, "as") || alias.tokens().count() == 1)
            .ok_or_else(|| Unsupported("alias".into()))?;
        self.ident(token)
    }

    fn table_alias(&self, alias: &SyntaxNode) -> Result<TableAlias> {
        let explicit = alias.tokens().count() > 1;
        let columns = if let Some(list) = alias.child(SyntaxKind::ColumnList) {
            self.column_list(list)?
                .into_iter()
                .map(|name| TableAliasColumnDef {
                    name,
                    data_type: None,
                })
                .collect()
        } else if let Some(defs) = alias.child(SyntaxKind::TableElements) {
            defs.nodes()
                .map(|def| {
                    let (Some(name), Some(data_type)) =
                        (def.tokens().next(), def.child(SyntaxKind::TypeName))
                    else {
                        return unsupported("column definition");
                    };
                    Ok(TableAliasColumnDef {
                        name: self.ident(name)?,
                        data_type: Some(self.data_type(data_type)?),
                    })
                })
                .collect::<Result<_>>()?
        } else {
            vec![]
        };
        Ok(TableAlias {
            explicit,
            name: self.alias_ident(alias)?,
            columns,
            at: None,
        })
    }

    fn column_list(&self, list: &SyntaxNode) -> Result<Vec<Ident>> {
        list.tokens()
            .filter(|t| matches!(t.kind, SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            .map(|t| self.ident(t))
            .collect()
    }

    fn table_with_joins(&self, node: &SyntaxNode) -> Result<TableWithJoins> {
        let mut joins = vec![];
        let mut current = node;
        while current.kind == SyntaxKind::JoinExpr {
            let mut parts = current.nodes();
            let left = parts.next().ok_or_else(|| Unsupported("join".into()))?;
            let right = parts.next().ok_or_else(|| Unsupported("join".into()))?;
            let condition = parts.next();
            joins.push(self.join(current, right, condition)?);
            current = left;
        }
        joins.reverse();
        Ok(TableWithJoins {
            relation: self.table_factor(current)?,
            joins,
        })
    }

    fn join(
        &self,
        node: &SyntaxNode,
        right: &SyntaxNode,
        condition: Option<&SyntaxNode>,
    ) -> Result<Join> {
        let constraint = match condition {
            Some(c) if c.tokens().next().is_some_and(|t| self.is_kw(t, "on")) => {
                JoinConstraint::On(self.expr(expr_child(c)?)?)
            }
            Some(c) => {
                let list = c
                    .child(SyntaxKind::ColumnList)
                    .ok_or_else(|| Unsupported("USING".into()))?;
                JoinConstraint::Using(
                    self.column_list(list)?
                        .into_iter()
                        .map(|i| object_name(vec![i]))
                        .collect(),
                )
            }
            None if self.has_kw(node, "natural") => JoinConstraint::Natural,
            None => JoinConstraint::None,
        };
        let outer = self.has_kw(node, "outer");
        let join_operator = if self.has_kw(node, "cross") {
            JoinOperator::CrossJoin(constraint)
        } else if self.has_kw(node, "left") {
            if outer {
                JoinOperator::LeftOuter(constraint)
            } else {
                JoinOperator::Left(constraint)
            }
        } else if self.has_kw(node, "right") {
            if outer {
                JoinOperator::RightOuter(constraint)
            } else {
                JoinOperator::Right(constraint)
            }
        } else if self.has_kw(node, "full") {
            JoinOperator::FullOuter(constraint)
        } else if self.has_kw(node, "inner") {
            JoinOperator::Inner(constraint)
        } else {
            JoinOperator::Join(constraint)
        };
        Ok(Join {
            relation: self.table_factor(right)?,
            global: false,
            join_operator,
        })
    }

    fn table_factor(&self, node: &SyntaxNode) -> Result<TableFactor> {
        let lateral = node
            .tokens()
            .next()
            .is_some_and(|t| self.is_kw(t, "lateral"));
        let alias = node
            .child(SyntaxKind::Alias)
            .map(|a| self.table_alias(a))
            .transpose()?;
        match node.kind {
            SyntaxKind::TableRef => {
                let name = node
                    .child(SyntaxKind::Name)
                    .ok_or_else(|| Unsupported("table name".into()))?;
                Ok(TableFactor::Table {
                    name: object_name(self.name_parts(name)?),
                    alias,
                    args: None,
                    with_hints: vec![],
                    version: None,
                    with_ordinality: false,
                    partitions: vec![],
                    json_path: None,
                    sample: None,
                    index_hints: vec![],
                })
            }
            SyntaxKind::FuncTableRef => {
                let name_node = node
                    .child(SyntaxKind::Name)
                    .ok_or_else(|| Unsupported("function name".into()))?;
                if name_node.tokens().count() == 1
                    && name_node.tokens().any(|t| {
                        ["unnest", "json_table", "xmltable", "openjson"]
                            .iter()
                            .any(|n| self.is_kw(t, n))
                    })
                {
                    return unsupported(format!("{} in FROM", name_node.text(self.source)));
                }
                let name = object_name(
                    self.name_parts(
                        node.child(SyntaxKind::Name)
                            .ok_or_else(|| Unsupported("function name".into()))?,
                    )?,
                );
                let args_node = node
                    .child(SyntaxKind::ArgList)
                    .ok_or_else(|| Unsupported("arguments".into()))?;
                let (args, duplicate_treatment, clauses) = self.function_args(args_node)?;
                if duplicate_treatment.is_some() || !clauses.is_empty() {
                    return unsupported("aggregate arguments in FROM");
                }
                let with_ordinality = self.has_kw(node, "ordinality");
                if lateral {
                    Ok(TableFactor::Function {
                        lateral,
                        name,
                        args,
                        with_ordinality,
                        alias,
                    })
                } else {
                    Ok(TableFactor::Table {
                        name,
                        alias,
                        args: Some(ast::TableFunctionArgs {
                            args,
                            settings: None,
                        }),
                        with_hints: vec![],
                        version: None,
                        with_ordinality,
                        partitions: vec![],
                        json_path: None,
                        sample: None,
                        index_hints: vec![],
                    })
                }
            }
            SyntaxKind::SubqueryRef => {
                let query = node
                    .child(SyntaxKind::SelectStmt)
                    .ok_or_else(|| Unsupported("subquery".into()))?;
                Ok(TableFactor::Derived {
                    lateral,
                    subquery: Box::new(self.query(query)?),
                    alias,
                    sample: None,
                })
            }
            SyntaxKind::ParenFromItem => {
                let inner = node
                    .nodes()
                    .find(|n| n.kind != SyntaxKind::Alias)
                    .ok_or_else(|| Unsupported("parenthesized FROM item".into()))?;
                Ok(TableFactor::NestedJoin {
                    table_with_joins: Box::new(self.table_with_joins(inner)?),
                    alias,
                })
            }
            kind => unsupported(format!("{kind:?} in FROM")),
        }
    }

    fn order_by(&self, node: &SyntaxNode) -> Result<OrderBy> {
        Ok(OrderBy {
            kind: OrderByKind::Expressions(self.order_by_exprs(node)?),
            interpolate: None,
        })
    }

    fn order_by_exprs(&self, node: &SyntaxNode) -> Result<Vec<OrderByExpr>> {
        let mut exprs = vec![];
        for item in node.nodes() {
            let tokens: Vec<_> = item.tokens().collect();
            if tokens.iter().any(|t| self.is_kw(t, "using")) {
                return unsupported("ORDER BY ... USING");
            }
            let sort = if tokens.iter().any(|t| self.is_kw(t, "asc")) {
                Some(ast::OrderBySort::Asc)
            } else if tokens.iter().any(|t| self.is_kw(t, "desc")) {
                Some(ast::OrderBySort::Desc)
            } else {
                None
            };
            let nulls_first = if tokens.iter().any(|t| self.is_kw(t, "first")) {
                Some(true)
            } else if tokens.iter().any(|t| self.is_kw(t, "last")) {
                Some(false)
            } else {
                None
            };
            exprs.push(OrderByExpr {
                expr: self.expr(expr_child(item)?)?,
                options: OrderByOptions { sort, nulls_first },
                with_fill: None,
            });
        }
        Ok(exprs)
    }

    // --- expressions ---

    fn expr_list(&self, node: &SyntaxNode) -> Result<Vec<Expr>> {
        node.nodes().map(|n| self.expr(n)).collect()
    }

    fn name_parts(&self, name: &SyntaxNode) -> Result<Vec<Ident>> {
        name.tokens()
            .filter(|t| matches!(t.kind, SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            .map(|t| self.ident(t))
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    fn expr(&self, node: &SyntaxNode) -> Result<Expr> {
        let tokens: Vec<_> = node.tokens().collect();
        let children: Vec<_> = node.nodes().collect();
        let child = |i: usize| -> Result<&SyntaxNode> {
            children
                .get(i)
                .copied()
                .ok_or_else(|| Unsupported(format!("{:?}", node.kind)))
        };
        let negated = tokens.iter().any(|t| self.is_kw(t, "not"));
        Ok(match node.kind {
            SyntaxKind::Literal => Expr::Value(self.literal(tokens[0])?),
            SyntaxKind::ParamRef => Expr::Value(ValueWithSpan {
                value: Value::Placeholder(self.text(tokens[0]).to_string()),
                span: self.span(tokens[0].range),
            }),
            SyntaxKind::ColumnRef => {
                let name = child(0)?;
                if name.tokens().any(|t| t.kind == SyntaxKind::Operator) {
                    return unsupported("* in an expression");
                }
                let first = name.tokens().next().map_or("", |t| self.text(t));
                if name.tokens().count() == 1
                    && name.tokens().all(|t| t.kind == SyntaxKind::Ident)
                    && crate::grammar::is_value_keyword(first)
                {
                    if first.eq_ignore_ascii_case("system_user") {
                        return unsupported("SYSTEM_USER");
                    }
                    return Ok(Expr::Function(Function {
                        name: object_name(self.name_parts(name)?),
                        uses_odbc_syntax: false,
                        parameters: FunctionArguments::None,
                        args: FunctionArguments::None,
                        filter: None,
                        null_treatment: None,
                        over: None,
                        within_group: vec![],
                    }));
                }
                let mut parts = self.name_parts(name)?;
                if parts.len() == 1 {
                    Expr::Identifier(parts.remove(0))
                } else {
                    Expr::CompoundIdentifier(parts)
                }
            }
            SyntaxKind::BinaryExpr if child(1)?.kind == SyntaxKind::QuantifiedExpr => {
                let quantified = child(1)?;
                let op = tokens
                    .first()
                    .ok_or_else(|| Unsupported("operator".into()))?;
                let left = Box::new(self.expr(child(0)?)?);
                let compare_op = self.binary_operator(op)?;
                let inner = quantified
                    .nodes()
                    .next()
                    .ok_or_else(|| Unsupported("ANY / ALL".into()))?;
                let right = Box::new(if inner.kind == SyntaxKind::SelectStmt {
                    Expr::Subquery(Box::new(self.query(inner)?))
                } else {
                    self.expr(inner)?
                });
                let keyword = quantified
                    .tokens()
                    .next()
                    .map(|t| self.text(t).to_ascii_lowercase());
                if keyword.as_deref() == Some("all") {
                    Expr::AllOp {
                        left,
                        compare_op,
                        right,
                    }
                } else {
                    Expr::AnyOp {
                        left,
                        compare_op,
                        right,
                        is_some: keyword.as_deref() == Some("some"),
                    }
                }
            }
            SyntaxKind::BinaryExpr => {
                let op = tokens
                    .first()
                    .ok_or_else(|| Unsupported("operator".into()))?;
                Expr::BinaryOp {
                    left: Box::new(self.expr(child(0)?)?),
                    op: self.binary_operator(op)?,
                    right: Box::new(self.expr(child(1)?)?),
                }
            }
            SyntaxKind::PrefixExpr => {
                let op = tokens[0];
                let operand = child(0)?;
                if self.is_kw(op, "not") && operand.kind == SyntaxKind::ExistsExpr {
                    return Ok(Expr::Exists {
                        subquery: Box::new(self.query(child_query(operand)?)?),
                        negated: true,
                    });
                }
                let op = match self.text(op) {
                    "-" => UnaryOperator::Minus,
                    "+" => UnaryOperator::Plus,
                    "~" => UnaryOperator::BitwiseNot,
                    "@" => UnaryOperator::PGAbs,
                    "|/" => UnaryOperator::PGSquareRoot,
                    "||/" => UnaryOperator::PGCubeRoot,
                    "!!" => UnaryOperator::PGPrefixFactorial,
                    _ if self.is_kw(op, "not") => UnaryOperator::Not,
                    other => return unsupported(format!("prefix operator {other}")),
                };
                Expr::UnaryOp {
                    op,
                    expr: Box::new(self.expr(operand)?),
                }
            }
            SyntaxKind::IsExpr => {
                let expr = Box::new(self.expr(child(0)?)?);
                let last = tokens
                    .last()
                    .map(|t| self.text(t).to_ascii_lowercase())
                    .unwrap_or_default();
                if tokens.iter().any(|t| self.is_kw(t, "distinct")) {
                    let right = Box::new(self.expr(child(1)?)?);
                    if negated {
                        Expr::IsNotDistinctFrom(expr, right)
                    } else {
                        Expr::IsDistinctFrom(expr, right)
                    }
                } else {
                    match (last.as_str(), negated) {
                        ("null" | "isnull", false) => Expr::IsNull(expr),
                        ("null", true) | ("notnull", false) => Expr::IsNotNull(expr),
                        ("true", false) => Expr::IsTrue(expr),
                        ("true", true) => Expr::IsNotTrue(expr),
                        ("false", false) => Expr::IsFalse(expr),
                        ("false", true) => Expr::IsNotFalse(expr),
                        ("unknown", false) => Expr::IsUnknown(expr),
                        ("unknown", true) => Expr::IsNotUnknown(expr),
                        _ => return unsupported("IS"),
                    }
                }
            }
            SyntaxKind::InExpr => {
                let expr = Box::new(self.expr(child(0)?)?);
                let rhs = child(1)?;
                if rhs.kind == SyntaxKind::SubqueryExpr {
                    Expr::InSubquery {
                        expr,
                        subquery: Box::new(self.query(child_query(rhs)?)?),
                        negated,
                    }
                } else {
                    Expr::InList {
                        expr,
                        list: self.expr_list(rhs)?,
                        negated,
                    }
                }
            }
            SyntaxKind::BetweenExpr => {
                if tokens
                    .iter()
                    .any(|t| self.is_kw(t, "symmetric") || self.is_kw(t, "asymmetric"))
                {
                    return unsupported("BETWEEN SYMMETRIC");
                }
                Expr::Between {
                    expr: Box::new(self.expr(child(0)?)?),
                    negated,
                    low: Box::new(self.expr(child(1)?)?),
                    high: Box::new(self.expr(child(2)?)?),
                }
            }
            SyntaxKind::LikeExpr => {
                let expr = Box::new(self.expr(child(0)?)?);
                let pattern = Box::new(self.expr(child(1)?)?);
                let escape_char = children
                    .get(2)
                    .map(|e| self.expr(e).map(Box::new))
                    .transpose()?;
                if tokens.iter().any(|t| self.is_kw(t, "similar")) {
                    Expr::SimilarTo {
                        negated,
                        expr,
                        pattern,
                        escape_char: escape_char.clone(),
                    }
                } else if tokens.iter().any(|t| self.is_kw(t, "ilike")) {
                    Expr::ILike {
                        negated,
                        any: false,
                        expr,
                        pattern,
                        escape_char: escape_char.clone(),
                    }
                } else {
                    Expr::Like {
                        negated,
                        any: false,
                        expr,
                        pattern,
                        escape_char: escape_char.clone(),
                    }
                }
            }
            SyntaxKind::ParenExpr => Expr::Nested(Box::new(self.expr(child(0)?)?)),
            SyntaxKind::RowExpr => Expr::Tuple(self.expr_list(node)?),
            SyntaxKind::SubqueryExpr => Expr::Subquery(Box::new(self.query(child_query(node)?)?)),
            SyntaxKind::ExistsExpr => Expr::Exists {
                subquery: Box::new(self.query(child_query(node)?)?),
                negated: false,
            },
            SyntaxKind::CastExpr | SyntaxKind::TypeCastExpr => {
                let type_node = node
                    .child(SyntaxKind::TypeName)
                    .ok_or_else(|| Unsupported("type".into()))?;
                Expr::Cast {
                    kind: if node.kind == SyntaxKind::CastExpr {
                        CastKind::Cast
                    } else {
                        CastKind::DoubleColon
                    },
                    expr: Box::new(self.expr(child(0)?)?),
                    data_type: self.data_type(type_node)?,
                    format: None,
                }
            }
            SyntaxKind::CaseExpr => self.case(node, &tokens, &children)?,
            // sqlparser reads `DEFAULT` in VALUES and SET as an identifier.
            SyntaxKind::DefaultExpr => Expr::Identifier(self.ident(tokens[0])?),
            SyntaxKind::ArrayExpr => Expr::Array(ast::Array {
                elem: self.expr_list(node)?,
                named: tokens.first().is_some_and(|t| self.is_kw(t, "array")),
            }),
            SyntaxKind::ArraySubquery => Expr::Function(Function {
                name: object_name(vec![self.ident(tokens[0])?]),
                uses_odbc_syntax: false,
                parameters: FunctionArguments::None,
                args: FunctionArguments::Subquery(Box::new(self.query(child_query(node)?)?)),
                filter: None,
                null_treatment: None,
                over: None,
                within_group: vec![],
            }),
            SyntaxKind::TypedLiteral => {
                let type_node = child(0)?;
                let literal = child(1)?;
                if children.len() > 2 || !tokens.is_empty() {
                    return unsupported("interval fields");
                }
                let value = self.literal(
                    literal
                        .tokens()
                        .next()
                        .ok_or_else(|| Unsupported("literal".into()))?,
                )?;
                if type_node.text(self.source).starts_with('_') {
                    // `_int4 '{1}'` (an array type in PostgreSQL); sqlparser reads a MySQL
                    // charset introducer.
                    return unsupported("typed literal of a type starting with _");
                }
                if type_node.text(self.source).eq_ignore_ascii_case("interval") {
                    Expr::Interval(ast::Interval {
                        value: Box::new(Expr::Value(value)),
                        leading_field: None,
                        leading_precision: None,
                        last_field: None,
                        fractional_seconds_precision: None,
                    })
                } else {
                    Expr::TypedString(ast::TypedString {
                        data_type: self.data_type(type_node)?,
                        value,
                        uses_odbc_syntax: false,
                    })
                }
            }
            SyntaxKind::ExtractExpr => {
                let field = tokens.get(2).ok_or_else(|| Unsupported("EXTRACT".into()))?;
                Expr::Extract {
                    field: self.with_sqlparser(field.range, |p| p.parse_date_time_field())?,
                    syntax: ast::ExtractSyntax::From,
                    expr: Box::new(self.expr(child(0)?)?),
                }
            }
            SyntaxKind::CollateExpr => Expr::Collate {
                expr: Box::new(self.expr(child(0)?)?),
                collation: object_name(self.name_parts(child(1)?)?),
            },
            SyntaxKind::AtTimeZoneExpr => Expr::AtTimeZone {
                timestamp: Box::new(self.expr(child(0)?)?),
                time_zone: Box::new(self.expr(child(1)?)?),
            },
            SyntaxKind::FieldAccessExpr | SyntaxKind::SubscriptExpr => self.field_access(node)?,
            SyntaxKind::FuncCall => self.function(node)?,
            kind => return unsupported(format!("{kind:?}")),
        })
    }

    fn case(
        &self,
        node: &SyntaxNode,
        tokens: &[&SyntaxToken],
        children: &[&SyntaxNode],
    ) -> Result<Expr> {
        let operand = children
            .iter()
            .find(|c| !matches!(c.kind, SyntaxKind::WhenClause | SyntaxKind::ElseClause))
            .map(|c| self.expr(c).map(Box::new))
            .transpose()?;
        let mut conditions = vec![];
        let mut else_result = None;
        for c in children {
            match c.kind {
                SyntaxKind::WhenClause => {
                    let parts: Vec<_> = c.nodes().collect();
                    let [condition, result] = parts[..] else {
                        return unsupported("WHEN");
                    };
                    conditions.push(CaseWhen {
                        condition: self.expr(condition)?,
                        result: self.expr(result)?,
                    });
                }
                SyntaxKind::ElseClause => {
                    else_result = Some(Box::new(self.expr(expr_child(c)?)?));
                }
                _ => {}
            }
        }
        let end = all_tokens(node)
            .last()
            .copied()
            .ok_or_else(|| Unsupported("CASE".into()))?;
        Ok(Expr::Case {
            case_token: self.attached(tokens[0]),
            end_token: self.attached(end),
            operand,
            conditions,
            else_result,
        })
    }

    /// Whether `node` is a call to an unqualified, unquoted function named in `names`.
    fn is_call_to(&self, node: &SyntaxNode, names: &[&str]) -> bool {
        node.kind == SyntaxKind::FuncCall
            && node.child(SyntaxKind::Name).is_some_and(|name| {
                let tokens: Vec<_> = name.tokens().collect();
                tokens.len() == 1 && names.iter().any(|n| self.is_kw(tokens[0], n))
            })
    }

    fn function(&self, node: &SyntaxNode) -> Result<Expr> {
        let name = node
            .child(SyntaxKind::Name)
            .ok_or_else(|| Unsupported("function name".into()))?;
        if self.is_call_to(node, SPECIAL_FUNCTIONS) {
            return unsupported(format!("special form {}", name.text(self.source)));
        }
        let args_node = node
            .child(SyntaxKind::ArgList)
            .ok_or_else(|| Unsupported("arguments".into()))?;
        let (args, duplicate_treatment, clauses) = self.function_args(args_node)?;
        let within_group = match node.child(SyntaxKind::WithinGroupClause) {
            Some(c) => self.order_by_exprs(
                c.child(SyntaxKind::OrderByClause)
                    .ok_or_else(|| Unsupported("WITHIN GROUP".into()))?,
            )?,
            None => vec![],
        };
        let filter = node
            .child(SyntaxKind::FilterClause)
            .map(|c| expr_child(c).and_then(|e| self.expr(e)).map(Box::new))
            .transpose()?;
        let over = node
            .child(SyntaxKind::OverClause)
            .map(|c| self.over(c))
            .transpose()?;
        Ok(Expr::Function(Function {
            name: object_name(self.name_parts(name)?),
            uses_odbc_syntax: false,
            parameters: FunctionArguments::None,
            args: FunctionArguments::List(FunctionArgumentList {
                duplicate_treatment,
                args,
                clauses,
            }),
            filter,
            null_treatment: None,
            over,
            within_group,
        }))
    }

    #[allow(clippy::type_complexity)]
    fn function_args(
        &self,
        args_node: &SyntaxNode,
    ) -> Result<(
        Vec<FunctionArg>,
        Option<ast::DuplicateTreatment>,
        Vec<ast::FunctionArgumentClause>,
    )> {
        let mut args = vec![];
        let mut clauses = vec![];
        for arg in args_node.nodes() {
            match arg.kind {
                SyntaxKind::Star => args.push(FunctionArg::Unnamed(FunctionArgExpr::Wildcard)),
                SyntaxKind::OrderByClause => {
                    clauses.push(ast::FunctionArgumentClause::OrderBy(
                        self.order_by_exprs(arg)?,
                    ));
                }
                _ => args.push(FunctionArg::Unnamed(FunctionArgExpr::Expr(self.expr(arg)?))),
            }
        }
        let duplicate_treatment = if self.has_kw(args_node, "distinct") {
            Some(ast::DuplicateTreatment::Distinct)
        } else if self.has_kw(args_node, "all") {
            Some(ast::DuplicateTreatment::All)
        } else {
            None
        };
        Ok((args, duplicate_treatment, clauses))
    }

    fn over(&self, node: &SyntaxNode) -> Result<ast::WindowType> {
        if let Some(spec) = node.child(SyntaxKind::WindowSpec) {
            return Ok(ast::WindowType::WindowSpec(self.window_spec(spec)?));
        }
        let name = node
            .tokens()
            .nth(1)
            .ok_or_else(|| Unsupported("OVER".into()))?;
        Ok(ast::WindowType::NamedWindow(self.ident(name)?))
    }

    fn window_spec(&self, node: &SyntaxNode) -> Result<ast::WindowSpec> {
        let window_name = node
            .tokens()
            .nth(1)
            .filter(|t| matches!(t.kind, SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            .map(|t| self.ident(t))
            .transpose()?;
        let partition_by = match node.child(SyntaxKind::PartitionByClause) {
            Some(c) => self.expr_list(
                c.child(SyntaxKind::ExprList)
                    .ok_or_else(|| Unsupported("PARTITION BY".into()))?,
            )?,
            None => vec![],
        };
        let order_by = match node.child(SyntaxKind::OrderByClause) {
            Some(c) => self.order_by_exprs(c)?,
            None => vec![],
        };
        let window_frame = node
            .child(SyntaxKind::FrameClause)
            .map(|c| self.window_frame(c))
            .transpose()?;
        Ok(ast::WindowSpec {
            window_name,
            partition_by,
            order_by,
            window_frame,
        })
    }

    fn window_frame(&self, node: &SyntaxNode) -> Result<ast::WindowFrame> {
        if self.has_kw(node, "exclude") {
            return unsupported("EXCLUDE");
        }
        let units = node
            .tokens()
            .next()
            .map(|t| self.text(t).to_ascii_lowercase());
        let units = match units.as_deref() {
            Some("rows") => ast::WindowFrameUnits::Rows,
            Some("range") => ast::WindowFrameUnits::Range,
            _ => ast::WindowFrameUnits::Groups,
        };
        let bounds: Vec<_> = node
            .nodes()
            .map(|b| self.frame_bound(b))
            .collect::<Result<_>>()?;
        let mut bounds = bounds.into_iter();
        let start_bound = bounds.next().ok_or_else(|| Unsupported("frame".into()))?;
        Ok(ast::WindowFrame {
            units,
            start_bound,
            end_bound: bounds.next(),
        })
    }

    fn frame_bound(&self, node: &SyntaxNode) -> Result<ast::WindowFrameBound> {
        if self.has_kw(node, "current") {
            return Ok(ast::WindowFrameBound::CurrentRow);
        }
        let value = node
            .nodes()
            .next()
            .map(|e| self.expr(e).map(Box::new))
            .transpose()?;
        if self.has_kw(node, "preceding") {
            Ok(ast::WindowFrameBound::Preceding(value))
        } else {
            Ok(ast::WindowFrameBound::Following(value))
        }
    }

    fn binary_operator(&self, op: &SyntaxToken) -> Result<BinaryOperator> {
        if self.is_kw(op, "and") {
            return Ok(BinaryOperator::And);
        }
        if self.is_kw(op, "or") {
            return Ok(BinaryOperator::Or);
        }
        Ok(match self.text(op) {
            "=" => BinaryOperator::Eq,
            "<>" | "!=" => BinaryOperator::NotEq,
            "<" => BinaryOperator::Lt,
            ">" => BinaryOperator::Gt,
            "<=" => BinaryOperator::LtEq,
            ">=" => BinaryOperator::GtEq,
            "+" => BinaryOperator::Plus,
            "-" => BinaryOperator::Minus,
            "*" => BinaryOperator::Multiply,
            "/" => BinaryOperator::Divide,
            "%" => BinaryOperator::Modulo,
            "^" => BinaryOperator::PGExp,
            "||" => BinaryOperator::StringConcat,
            "->" => BinaryOperator::Arrow,
            "->>" => BinaryOperator::LongArrow,
            "#>" => BinaryOperator::HashArrow,
            "#>>" => BinaryOperator::HashLongArrow,
            "@>" => BinaryOperator::AtArrow,
            "<@" => BinaryOperator::ArrowAt,
            "?" => BinaryOperator::Question,
            "?|" => BinaryOperator::QuestionPipe,
            "?&" => BinaryOperator::QuestionAnd,
            "~" => BinaryOperator::PGRegexMatch,
            "~*" => BinaryOperator::PGRegexIMatch,
            "!~" => BinaryOperator::PGRegexNotMatch,
            "!~*" => BinaryOperator::PGRegexNotIMatch,
            "&" => BinaryOperator::BitwiseAnd,
            "|" => BinaryOperator::BitwiseOr,
            "#" => BinaryOperator::PGBitwiseXor,
            "<<" => BinaryOperator::PGBitwiseShiftLeft,
            ">>" => BinaryOperator::PGBitwiseShiftRight,
            "&&" => BinaryOperator::PGOverlap,
            "@@" => BinaryOperator::AtAt,
            other => return unsupported(format!("operator {other}")),
        })
    }

    fn literal(&self, token: &SyntaxToken) -> Result<ValueWithSpan> {
        let text = self.text(token);
        let value = match token.kind {
            SyntaxKind::Number => Value::Number(text.to_string(), false),
            SyntaxKind::String => Value::SingleQuotedString(unquote(text, 0)),
            SyntaxKind::NationalString => Value::NationalStringLiteral(unquote(text, 1)),
            SyntaxKind::DollarString => {
                let tag_end = text[1..].find('$').map_or(1, |i| i + 2);
                let tag = &text[1..tag_end - 1];
                Value::DollarQuotedString(ast::DollarQuotedString {
                    value: text[tag_end..text.len() - tag_end].to_string(),
                    tag: (!tag.is_empty()).then(|| tag.to_string()),
                })
            }
            SyntaxKind::Ident if text.eq_ignore_ascii_case("true") => Value::Boolean(true),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("false") => Value::Boolean(false),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("null") => Value::Null,
            SyntaxKind::EscapeString
            | SyntaxKind::BitString
            | SyntaxKind::HexString
            | SyntaxKind::UnicodeString => {
                return self.with_sqlparser(token.range, |p| p.parse_value());
            }
            kind => return unsupported(format!("{kind:?} literal")),
        };
        Ok(ValueWithSpan {
            value,
            span: self.span(token.range),
        })
    }

    /// Type names are leaves for the analyzer: sqlparser parses the text of the node.
    fn data_type(&self, node: &SyntaxNode) -> Result<ast::DataType> {
        self.with_sqlparser(node.range, |p| p.parse_data_type())
    }

    /// Runs a sqlparser parse function on the text of `range` (a leaf such as a type name or
    /// an `EXTRACT` field), which must consume it all. The text before it is blanked out
    /// (same lines and character counts), so the spans in the result are the same as in
    /// the whole statement.
    fn with_sqlparser<T>(
        &self,
        range: TextRange,
        f: impl FnOnce(&mut sqlparser::parser::Parser<'_>) -> std::result::Result<T, ParserError>,
    ) -> Result<T> {
        let text = &self.source[range.to_range()];
        let fail = |e: ParserError| Unsupported(format!("{text}: {e}"));
        let tokens = self.sqlparser_tokens(range)?;
        let dialect = PostgreSqlDialect {};
        let mut p = sqlparser::parser::Parser::new(&dialect).with_tokens_with_locations(tokens);
        let value = f(&mut p).map_err(fail)?;
        p.expect_token(&Token::EOF).map_err(fail)?;
        Ok(value)
    }

    /// sqlparser's tokens for the text of `range`, with spans in the whole statement.
    fn sqlparser_tokens(&self, range: TextRange) -> Result<Vec<TokenWithSpan>> {
        let text = &self.source[range.to_range()];
        let mut tokens = sqlparser::tokenizer::Tokenizer::new(&PostgreSqlDialect {}, text)
            .tokenize_with_location()
            .map_err(|e| Unsupported(format!("{text}: {e}")))?;
        // Moved to where the text is in the statement.
        let origin = self.lines.location(range.start);
        let shift = |l: &mut Location| {
            if l.line == 1 {
                l.column += origin.column - 1;
            }
            l.line += origin.line - 1;
        };
        for token in &mut tokens {
            shift(&mut token.span.start);
            shift(&mut token.span.end);
        }
        Ok(tokens)
    }

    /// `(x).a.b`, `a[1][2:3]`, `t.a[1]`: one access chain on the innermost value.
    fn field_access(&self, node: &SyntaxNode) -> Result<Expr> {
        let mut chain = vec![];
        let mut current = node;
        loop {
            match current.kind {
                SyntaxKind::FieldAccessExpr => {
                    let field = current
                        .tokens()
                        .last()
                        .ok_or_else(|| Unsupported("field".into()))?;
                    if field.kind == SyntaxKind::Operator {
                        return unsupported("(x).*");
                    }
                    chain.push(ast::AccessExpr::Dot(Expr::Identifier(self.ident(field)?)));
                }
                SyntaxKind::SubscriptExpr => {
                    chain.push(ast::AccessExpr::Subscript(self.subscript(current)?));
                }
                _ => break,
            }
            current = current
                .nodes()
                .next()
                .ok_or_else(|| Unsupported("access".into()))?;
        }
        chain.reverse();
        // `t.a[1]`: sqlparser keeps the first name as the root and the rest as fields.
        if current.kind == SyntaxKind::ColumnRef {
            if let Some(name) = current.child(SyntaxKind::Name) {
                let mut parts = self.name_parts(name)?;
                if parts.len() > 1 {
                    let rest: Vec<_> = parts
                        .drain(1..)
                        .map(|i| ast::AccessExpr::Dot(Expr::Identifier(i)))
                        .collect();
                    chain.splice(0..0, rest);
                    return Ok(Expr::CompoundFieldAccess {
                        root: Box::new(Expr::Identifier(parts.remove(0))),
                        access_chain: chain,
                    });
                }
            }
        }
        Ok(Expr::CompoundFieldAccess {
            root: Box::new(self.expr(current)?),
            access_chain: chain,
        })
    }

    fn subscript(&self, node: &SyntaxNode) -> Result<ast::Subscript> {
        let colon = node.tokens().find(|t| t.kind == SyntaxKind::Colon);
        let mut bounds = node.nodes().skip(1);
        let Some(colon) = colon else {
            let index = bounds
                .next()
                .ok_or_else(|| Unsupported("subscript".into()))?;
            return Ok(ast::Subscript::Index {
                index: self.expr(index)?,
            });
        };
        let mut lower_bound = None;
        let mut upper_bound = None;
        for bound in bounds {
            let expr = self.expr(bound)?;
            if bound.range.start < colon.range.start {
                lower_bound = Some(expr);
            } else {
                upper_bound = Some(expr);
            }
        }
        Ok(ast::Subscript::Slice {
            lower_bound,
            upper_bound,
            stride: None,
        })
    }
}

/// The text of a quoted string without its prefix (`prefix` bytes, as in `N'...'`), its
/// quotes and doubled quotes.
fn unquote(text: &str, prefix: usize) -> String {
    let inner = &text[prefix..];
    let inner = inner
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(inner);
    inner.replace("''", "'")
}

fn plain_table(name: ObjectName, alias: Option<TableAlias>) -> TableFactor {
    TableFactor::Table {
        name,
        alias,
        args: None,
        with_hints: vec![],
        version: None,
        with_ordinality: false,
        partitions: vec![],
        json_path: None,
        sample: None,
        index_hints: vec![],
    }
}

fn object_name(parts: Vec<Ident>) -> ObjectName {
    ObjectName(parts.into_iter().map(ObjectNamePart::Identifier).collect())
}

/// The single expression node of a clause.
fn expr_child<'a>(node: &'a SyntaxNode<'a>) -> Result<&'a SyntaxNode<'a>> {
    node.nodes()
        .find(|n| n.kind != SyntaxKind::Alias)
        .ok_or_else(|| Unsupported(format!("{:?} without an expression", node.kind)))
}

fn child_query<'a>(node: &'a SyntaxNode<'a>) -> Result<&'a SyntaxNode<'a>> {
    node.child(SyntaxKind::SelectStmt)
        .ok_or_else(|| Unsupported("subquery".into()))
}
