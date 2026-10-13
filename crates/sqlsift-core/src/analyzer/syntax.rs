//! Experimental (#257): checks that read the in-house parser's syntax tree directly
//! instead of sqlparser's AST, moved over one at a time.
//!
//! A statement the in-house parser read comes with its tree (see
//! [`super::Analyzer::parse_statements`]); the checks here then replace their AST
//! versions for it, and must report the same diagnostics. Without the `inhouse-parser`
//! feature, statements never have a tree.

#[cfg(feature = "inhouse-parser")]
pub(super) use sqlsift_parser::{Arena, SyntaxNode};

/// Stand-in for the in-house parser's arena when it isn't built in.
#[cfg(not(feature = "inhouse-parser"))]
#[derive(Default)]
pub(super) struct Arena;

#[cfg(not(feature = "inhouse-parser"))]
impl Arena {
    pub(super) fn new() -> Self {
        Arena
    }
}

/// Stand-in for a syntax tree node when the in-house parser isn't built in: there is none.
#[cfg(not(feature = "inhouse-parser"))]
pub(super) enum SyntaxNode<'t> {
    #[allow(dead_code)]
    Never(std::convert::Infallible, std::marker::PhantomData<&'t ()>),
}

/// A statement's tree, if the in-house parser read it.
pub(super) type Syntax<'t> = Option<&'t SyntaxNode<'t>>;

#[cfg(feature = "inhouse-parser")]
pub(super) use checks::restrictions;

#[cfg(feature = "inhouse-parser")]
mod checks {
    use sqlsift_parser::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

    use super::super::restriction::{
        insert_without_columns, limit_without_order_by, select_star, unfiltered_delete,
        unfiltered_update,
    };
    use super::super::LineIndex;
    use crate::error::{Diagnostic, Span};

    /// The text a tree was parsed from, to read tokens and locate them.
    struct Source<'a> {
        text: &'a str,
        lines: &'a LineIndex<'a>,
    }

    impl Source<'_> {
        fn is_kw(&self, token: &SyntaxToken, kw: &str) -> bool {
            token.kind == SyntaxKind::Ident && token.text(self.text).eq_ignore_ascii_case(kw)
        }

        /// The span sqlparser gives the text in `range` (length in characters on its
        /// first line, 1 if it ends on an earlier column of a later line).
        fn span(&self, range: TextRange) -> Span {
            let (line, column) = self.lines.line_column(range.start as usize);
            let (_, end_column) = self.lines.line_column(range.end as usize);
            let length = if end_column > column {
                end_column - column
            } else {
                1
            };
            Span::with_location(line, column, length)
        }
    }

    /// E0024-E0026 and E0028 (the restriction rules) for one statement, from its tree.
    /// Mirrors `Resolver::check_restrictions`.
    pub(in crate::analyzer) fn restrictions(
        text: &str,
        lines: &LineIndex,
        stmt: &SyntaxNode,
    ) -> Vec<Diagnostic> {
        let source = Source { text, lines };
        let mut out = vec![];
        statement_restrictions(&source, stmt, &mut out);
        unordered_limits(&source, stmt, false, &mut out);
        out
    }

    /// E0024-E0026 for a statement, and for the `INSERT` / `UPDATE` / `DELETE`
    /// statements in it (`WITH ... INSERT`, data-modifying CTEs).
    fn statement_restrictions(source: &Source, stmt: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        let table = || {
            stmt.child(SyntaxKind::Name)
                .and_then(last_name_part)
                .map(|t| source.span(t.range))
        };
        let filtered = stmt.child(SyntaxKind::WhereClause).is_some();
        match stmt.kind {
            SyntaxKind::UpdateStmt if !filtered => out.push(unfiltered_update(table())),
            SyntaxKind::DeleteStmt if !filtered => out.push(unfiltered_delete(table())),
            SyntaxKind::InsertStmt => {
                // `INSERT INTO t DEFAULT VALUES` has no query
                if let Some(query) = stmt.child(SyntaxKind::SelectStmt) {
                    if stmt.child(SyntaxKind::ColumnList).is_none() {
                        out.push(insert_without_columns(table()));
                    }
                    select_stars(source, query, out);
                }
            }
            SyntaxKind::SelectStmt => select_stars(source, stmt, out),
            _ => {}
        }
        nested_statements(source, stmt, out);
    }

    fn nested_statements(source: &Source, node: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        for child in node.nodes() {
            match child.kind {
                SyntaxKind::InsertStmt | SyntaxKind::UpdateStmt | SyntaxKind::DeleteStmt => {
                    statement_restrictions(source, child, out);
                }
                _ => nested_statements(source, child, out),
            }
        }
    }

    /// The last part of a dotted name.
    fn last_name_part<'a>(name: &'a SyntaxNode<'a>) -> Option<&'a SyntaxToken> {
        name.tokens()
            .filter(|t| matches!(t.kind, SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            .last()
    }

    /// The body of a query (`SelectStmt`): a select, set operation, parenthesized query,
    /// `VALUES` or data-modifying statement.
    fn query_body<'a>(query: &'a SyntaxNode<'a>) -> Option<&'a SyntaxNode<'a>> {
        query.nodes().find(|n| {
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
    }

    /// E0026 for the wildcards in the result columns of `query` (not its subqueries).
    fn select_stars(source: &Source, query: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        if let Some(body) = query_body(query) {
            body_stars(source, body, out);
        }
    }

    /// [`select_stars`] for a query body, or a side of a set operation.
    fn body_stars(source: &Source, body: &SyntaxNode, out: &mut Vec<Diagnostic>) {
        match body.kind {
            SyntaxKind::SimpleSelect => {
                let items = body
                    .child(SyntaxKind::SelectClause)
                    .and_then(|c| c.child(SyntaxKind::TargetList))
                    .into_iter()
                    .flat_map(SyntaxNode::nodes);
                for item in items {
                    if let Some(star) = wildcard(item) {
                        out.push(select_star(Some(source.span(star.range))));
                    }
                }
            }
            SyntaxKind::SetOperation => {
                for side in body.nodes() {
                    body_stars(source, side, out);
                }
            }
            SyntaxKind::ParenSelect => {
                if let Some(inner) = body.child(SyntaxKind::SelectStmt) {
                    select_stars(source, inner, out);
                }
            }
            _ => {}
        }
    }

    /// The `*` of a result column that is `*` or `t.*`.
    fn wildcard<'a>(item: &'a SyntaxNode<'a>) -> Option<&'a SyntaxToken> {
        if let Some(star) = item.child(SyntaxKind::Star) {
            return star.tokens().next();
        }
        let expr = item.nodes().next()?;
        if expr.kind != SyntaxKind::ColumnRef {
            return None;
        }
        expr.child(SyntaxKind::Name)?
            .tokens()
            .find(|t| t.kind == SyntaxKind::Operator)
    }

    /// E0028 for the queries in `node`, subqueries and CTEs too, except the query of an
    /// `EXISTS` (`exists`: `node` is, or is the parentheses around, that query).
    fn unordered_limits(
        source: &Source,
        node: &SyntaxNode,
        exists: bool,
        out: &mut Vec<Diagnostic>,
    ) {
        if node.kind == SyntaxKind::SelectStmt && !exists && !is_ordered(node) {
            if let Some((keyword, value)) = limiting_clause(source, node) {
                out.push(limit_without_order_by(keyword, Some(source.span(value))));
            }
        }
        for child in node.nodes() {
            let exists = match (node.kind, child.kind) {
                (SyntaxKind::ExistsExpr, SyntaxKind::SelectStmt) => true,
                // `EXISTS ((SELECT ...))`: the query in the parentheses is the EXISTS one
                (SyntaxKind::SelectStmt, SyntaxKind::ParenSelect)
                | (SyntaxKind::ParenSelect, SyntaxKind::SelectStmt) => exists,
                _ => false,
            };
            unordered_limits(source, child, exists, out);
        }
    }

    /// Whether `query` has an `ORDER BY`, its own or that of the parenthesized query it
    /// limits: `(SELECT ... ORDER BY id) LIMIT 5`
    fn is_ordered(query: &SyntaxNode) -> bool {
        query.child(SyntaxKind::OrderByClause).is_some()
            || query_body(query)
                .filter(|b| b.kind == SyntaxKind::ParenSelect)
                .and_then(|b| b.child(SyntaxKind::SelectStmt))
                .is_some_and(is_ordered)
    }

    /// The `LIMIT` (not `LIMIT ALL`) or else the `OFFSET` of `query`: its keyword and the
    /// range sqlparser gives its value.
    fn limiting_clause(source: &Source, query: &SyntaxNode) -> Option<(&'static str, TextRange)> {
        let value = |clause: &SyntaxNode| clause.nodes().next().map(value_range);
        let limit = query
            .child(SyntaxKind::LimitClause)
            .filter(|c| !c.tokens().any(|t| source.is_kw(t, "all")))
            .and_then(value);
        if let Some(range) = limit {
            return Some(("LIMIT", range));
        }
        let offset = query.child(SyntaxKind::OffsetClause).and_then(value)?;
        Some(("OFFSET", offset))
    }

    /// The range sqlparser gives an expression: without its parentheses, and without the
    /// operator of a prefix expression (`-1` is located at `1`).
    fn value_range(expr: &SyntaxNode) -> TextRange {
        match expr.kind {
            SyntaxKind::ParenExpr | SyntaxKind::PrefixExpr => {
                expr.nodes().next().map_or(expr.range, value_range)
            }
            _ => expr.range,
        }
    }
}
