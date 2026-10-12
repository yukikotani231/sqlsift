//! Differential testing against sqlparser: each statement is parsed by both parsers and
//! the lowered AST compared with sqlparser's, first by structure (sqlparser's `==` ignores
//! spans), then with spans (the `Debug` output includes them).

use sqlparser::dialect::PostgreSqlDialect;

use crate::lower::lower_statement;
use crate::{parse, SyntaxKind};

/// How one statement compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Same AST, same spans.
    Same,
    /// Same AST, but some span differs.
    SpansDiffer { ours: String, theirs: String },
    /// Both parsers read it, with different ASTs: a bug in this parser or the lowering.
    Different { ours: String, theirs: String },
    /// sqlparser reads it; this parser (or the lowering) doesn't handle it yet.
    Unsupported(String),
    /// Only this parser reads it.
    OnlyOurs,
    /// Neither parser reads it.
    Neither,
}

impl Outcome {
    /// A short name for summaries.
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Same => "same",
            Outcome::SpansDiffer { .. } => "spans differ",
            Outcome::Different { .. } => "different",
            Outcome::Unsupported(_) => "unsupported",
            Outcome::OnlyOurs => "only ours",
            Outcome::Neither => "neither",
        }
    }
}

/// One statement of the input and how it compared.
#[derive(Debug, Clone)]
pub struct StatementDiff {
    /// The statement's byte offset in the input.
    pub offset: usize,
    pub text: String,
    pub outcome: Outcome,
}

/// Splits `source` into statements at the top-level `;` of this parser's tree (so text the
/// parser couldn't place stays with its statement) and compares each one.
pub fn compare_source(source: &str) -> Vec<StatementDiff> {
    let parsed = parse(source);
    let mut out = vec![];
    let mut start: Option<u32> = None;
    let mut end = 0;
    let mut flush = |start: &mut Option<u32>, end: u32| {
        if let Some(s) = start.take() {
            let text = &source[s as usize..end as usize];
            out.push(StatementDiff {
                offset: s as usize,
                text: text.to_string(),
                outcome: compare_statement(text),
            });
        }
    };
    for child in &parsed.root.children {
        let kind = child.kind();
        if kind == SyntaxKind::Semicolon {
            flush(&mut start, end);
        } else if !kind.is_trivia() {
            start.get_or_insert(child.range().start);
            end = child.range().end;
        }
    }
    flush(&mut start, end);
    out
}

/// Compares the two parsers on a single statement.
pub fn compare_statement(text: &str) -> Outcome {
    let theirs = sqlparser::parser::Parser::parse_sql(&PostgreSqlDialect {}, text)
        .ok()
        .filter(|stmts| stmts.len() == 1)
        .map(|mut stmts| stmts.remove(0));
    let parsed = parse(text);
    let mut stmts = parsed.statements();
    let ours = match (stmts.next(), stmts.next()) {
        (Some(stmt), None) if parsed.errors.is_empty() => {
            lower_statement(text, stmt).map_err(|e| e.0)
        }
        (Some(_), None) => Err(format!(
            "syntax error: {}",
            parsed.errors.first().map_or("", |e| e.message.as_str())
        )),
        _ => Err("not one statement".to_string()),
    };
    match (ours, theirs) {
        (Ok(ours), Some(theirs)) => {
            let same = ours == theirs;
            let (ours, theirs) = (format!("{ours:#?}"), format!("{theirs:#?}"));
            if !same {
                Outcome::Different { ours, theirs }
            } else if ours == theirs {
                Outcome::Same
            } else {
                Outcome::SpansDiffer { ours, theirs }
            }
        }
        (Err(reason), Some(_)) => Outcome::Unsupported(reason),
        (Ok(_), None) => Outcome::OnlyOurs,
        (Err(_), None) => Outcome::Neither,
    }
}

/// The first differing line of two `Debug` dumps, with a few lines of context, for reports.
pub fn first_difference(ours: &str, theirs: &str) -> String {
    let a: Vec<_> = ours.lines().collect();
    let b: Vec<_> = theirs.lines().collect();
    let i = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let from = i.saturating_sub(6);
    let mut out = String::new();
    for line in &a[from..i] {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    for (prefix, side) in [("- ours  ", &a), ("+ theirs", &b)] {
        for line in side.iter().skip(i).take(3) {
            out.push_str(prefix);
            out.push(' ');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
