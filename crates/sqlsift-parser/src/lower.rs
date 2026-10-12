//! Lowering: turns statements of the syntax tree into sqlparser's AST, so the analyzer can
//! run on this parser's output unchanged while the parser is being built.
//!
//! This is a temporary bridge (see #257): the analyzer is meant to read the new tree
//! directly later. A construct the lowering doesn't handle yet gives
//! [`Unsupported`], never a guess, so a statement either lowers to exactly what sqlparser
//! would produce or not at all. The differential test checks that.

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
use sqlparser::tokenizer::{Location, Span, Token, TokenWithSpan};

use crate::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

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
    match node.kind {
        SyntaxKind::SelectStmt => Ok(Statement::Query(Box::new(cx.query(node)?))),
        SyntaxKind::OpaqueStmt => unsupported("statement"),
        kind => unsupported(format!("{kind:?}")),
    }
}

struct Lowering<'a> {
    source: &'a str,
    lines: LineIndex<'a>,
}

/// The non-trivia tokens of a node, children's included.
fn all_tokens(node: &SyntaxNode) -> Vec<&SyntaxToken> {
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
        for child in node.nodes() {
            match child.kind {
                SyntaxKind::OrderByClause => order_by = Some(self.order_by(child)?),
                SyntaxKind::LimitClause => {
                    if child.tokens().any(|t| self.is_kw(t, "all")) {
                        return unsupported("LIMIT ALL");
                    }
                    limit = Some(self.expr(expr_child(child)?)?);
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
                    SyntaxKind::SimpleSelect | SyntaxKind::SetOperation | SyntaxKind::ParenSelect
                )
            })
            .ok_or_else(|| Unsupported("query body".into()))?;
        Ok(Query {
            with,
            body: Box::new(self.set_expr(body)?),
            order_by,
            limit_clause,
            fetch: None,
            locks: vec![],
            for_clause: None,
            settings: None,
            format_clause: None,
            pipe_operators: vec![],
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
                query: Box::new(self.query(child_query(cte)?)?),
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
            named_window: vec![],
            qualify: None,
            window_before_qualify: false,
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
        let columns = match alias.child(SyntaxKind::ColumnList) {
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
            SyntaxKind::SubqueryRef => {
                let query = node
                    .child(SyntaxKind::SelectStmt)
                    .ok_or_else(|| Unsupported("subquery".into()))?;
                Ok(TableFactor::Derived {
                    lateral: false,
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
        Ok(OrderBy {
            kind: OrderByKind::Expressions(exprs),
            interpolate: None,
        })
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
                    return unsupported(first.to_ascii_uppercase());
                }
                let mut parts = self.name_parts(name)?;
                if parts.len() == 1 {
                    Expr::Identifier(parts.remove(0))
                } else {
                    Expr::CompoundIdentifier(parts)
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
                if children.len() > 2 {
                    return unsupported("ESCAPE");
                }
                let expr = Box::new(self.expr(child(0)?)?);
                let pattern = Box::new(self.expr(child(1)?)?);
                if tokens.iter().any(|t| self.is_kw(t, "similar")) {
                    Expr::SimilarTo {
                        negated,
                        expr,
                        pattern,
                        escape_char: None,
                    }
                } else if tokens.iter().any(|t| self.is_kw(t, "ilike")) {
                    Expr::ILike {
                        negated,
                        any: false,
                        expr,
                        pattern,
                        escape_char: None,
                    }
                } else {
                    Expr::Like {
                        negated,
                        any: false,
                        expr,
                        pattern,
                        escape_char: None,
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
        let mut args = vec![];
        if args_node.child(SyntaxKind::Star).is_some() {
            args.push(FunctionArg::Unnamed(FunctionArgExpr::Wildcard));
        }
        for arg in args_node.nodes().filter(|n| n.kind != SyntaxKind::Star) {
            args.push(FunctionArg::Unnamed(FunctionArgExpr::Expr(self.expr(arg)?)));
        }
        let duplicate_treatment = if self.has_kw(args_node, "distinct") {
            Some(ast::DuplicateTreatment::Distinct)
        } else if self.has_kw(args_node, "all") {
            Some(ast::DuplicateTreatment::All)
        } else {
            None
        };
        Ok(Expr::Function(Function {
            name: object_name(self.name_parts(name)?),
            uses_odbc_syntax: false,
            parameters: FunctionArguments::None,
            args: FunctionArguments::List(FunctionArgumentList {
                duplicate_treatment,
                args,
                clauses: vec![],
            }),
            filter: None,
            null_treatment: None,
            over: None,
            within_group: vec![],
        }))
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
            other => return unsupported(format!("operator {other}")),
        })
    }

    fn literal(&self, token: &SyntaxToken) -> Result<ValueWithSpan> {
        let text = self.text(token);
        let value = match token.kind {
            SyntaxKind::Number => Value::Number(text.to_string(), false),
            SyntaxKind::String => Value::SingleQuotedString(unquote(text, 0)),
            SyntaxKind::NationalString => Value::NationalStringLiteral(unquote(text, 1)),
            SyntaxKind::EscapeString if !text.contains('\\') => {
                Value::EscapedStringLiteral(unquote(text, 1))
            }
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
            kind => return unsupported(format!("{kind:?} literal")),
        };
        Ok(ValueWithSpan {
            value,
            span: self.span(token.range),
        })
    }

    /// Type names are leaves for the analyzer: sqlparser parses the text of the node. The
    /// text before it is blanked out (same lines and character counts), so the spans in the
    /// result are the same as in the whole statement.
    fn data_type(&self, node: &SyntaxNode) -> Result<ast::DataType> {
        let start = node.range.start as usize;
        let mut text: String = self.source[..start]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        text.push_str(node.text(self.source));
        sqlparser::parser::Parser::new(&PostgreSqlDialect {})
            .try_with_sql(&text)
            .and_then(|mut p| {
                let data_type = p.parse_data_type()?;
                p.expect_token(&Token::EOF)?;
                Ok(data_type)
            })
            .map_err(|e| Unsupported(format!("type: {e}")))
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

fn object_name(parts: Vec<Ident>) -> ObjectName {
    ObjectName(parts.into_iter().map(ObjectNamePart::Identifier).collect())
}

/// The single expression node of a clause.
fn expr_child(node: &SyntaxNode) -> Result<&SyntaxNode> {
    node.nodes()
        .find(|n| n.kind != SyntaxKind::Alias)
        .ok_or_else(|| Unsupported(format!("{:?} without an expression", node.kind)))
}

fn child_query(node: &SyntaxNode) -> Result<&SyntaxNode> {
    node.child(SyntaxKind::SelectStmt)
        .ok_or_else(|| Unsupported("subquery".into()))
}
