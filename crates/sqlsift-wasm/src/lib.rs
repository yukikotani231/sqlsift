//! WebAssembly bindings for sqlsift.
//!
//! This crate exposes a single entry point, [`check`], which builds a schema
//! catalog from DDL and validates a query against it — the same pipeline the
//! CLI and LSP use — and returns the diagnostics as a JSON string. It powers
//! the browser playground in `site/`.
//!
//! The pure-Rust [`check_json`] / [`check_report`] functions contain all the
//! logic so they can be unit tested natively; the `#[wasm_bindgen]` wrapper is
//! a thin shim around them.

use serde::Serialize;
use sqlsift_core::schema::SchemaBuilder;
use sqlsift_core::{Analyzer, Diagnostic, DiagnosticKind, Severity, SqlDialect};
use wasm_bindgen::prelude::*;

/// Which editor a diagnostic belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Schema,
    Query,
}

/// A diagnostic flattened into an editor-friendly shape.
///
/// Positions are 1-indexed and measured in characters (Unicode scalar values),
/// matching sqlparser's tokenizer. `end_column` is exclusive. When a location
/// is unknown, all four position fields are `null`.
#[derive(Debug, Clone, Serialize)]
pub struct JsDiagnostic {
    pub code: &'static str,
    pub name: &'static str,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub source: Source,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
}

/// Result of a [`check_report`] call.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub diagnostics: Vec<JsDiagnostic>,
    pub schema_warnings: usize,
}

/// Validate `query_sql` against the schema defined by `schema_sql`.
///
/// `dialect` is one of `postgresql`, `mysql`, `sqlite` (aliases accepted by
/// the CLI also work); unknown values fall back to PostgreSQL.
///
/// Returns a JSON string:
///
/// ```json
/// { "diagnostics": [{ "code": "E0002", "name": "column-not-found",
///     "severity": "error", "message": "...", "help": "..." | null,
///     "source": "schema" | "query", "line": 1, "column": 8,
///     "end_line": 1, "end_column": 12 }],
///   "schema_warnings": 0 }
/// ```
#[wasm_bindgen]
pub fn check(schema_sql: &str, query_sql: &str, dialect: &str) -> String {
    check_json(schema_sql, query_sql, dialect)
}

/// The sqlsift version this module was built from (shared workspace version).
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Same as [`check`], usable from native Rust.
pub fn check_json(schema_sql: &str, query_sql: &str, dialect: &str) -> String {
    let report = check_report(schema_sql, query_sql, dialect);
    serde_json::to_string(&report).unwrap_or_else(|e| {
        format!(
            r#"{{"diagnostics":[],"schema_warnings":0,"error":{}}}"#,
            serde_json::Value::String(e.to_string())
        )
    })
}

/// Run the schema build + query analysis and return a structured report.
pub fn check_report(schema_sql: &str, query_sql: &str, dialect: &str) -> Report {
    let dialect: SqlDialect = dialect.parse().unwrap_or_default();
    let mut diagnostics = Vec::new();

    // --- Schema -----------------------------------------------------------
    let mut builder = SchemaBuilder::with_dialect(dialect);
    let mut schema_warnings = 0;

    if let Err(diags) = builder.parse(schema_sql) {
        diagnostics.extend(diags.iter().map(|d| convert(d, Source::Schema)));
    }

    let (catalog, schema_diags) = builder.build();
    schema_warnings += schema_diags.len();
    diagnostics.extend(schema_diags.iter().map(|d| convert(d, Source::Schema)));

    // --- Query ------------------------------------------------------------
    if !query_sql.trim().is_empty() {
        let mut analyzer = Analyzer::with_dialect(&catalog, dialect);
        // Inline `-- sqlsift:disable` directives are applied inside `analyze`.
        let query_diags = analyzer.analyze(query_sql);
        diagnostics.extend(query_diags.iter().map(|d| {
            let mut out = convert(d, Source::Query);
            // "found: EOF" errors carry no position at all; point at the end
            // of the input so the editor can still highlight something.
            if out.line.is_none() && d.kind == DiagnosticKind::ParseError {
                let (line, column) = end_of_input(query_sql);
                out.line = Some(line);
                out.column = Some(column);
                out.end_line = Some(line);
                out.end_column = Some(column + 1);
            }
            out
        }));
    }

    diagnostics.sort_by_key(|d| {
        (
            d.source,
            d.line.unwrap_or(usize::MAX),
            d.column.unwrap_or(usize::MAX),
        )
    });

    Report {
        diagnostics,
        schema_warnings,
    }
}

fn convert(diag: &Diagnostic, source: Source) -> JsDiagnostic {
    let mut out = JsDiagnostic {
        code: diag.code(),
        name: diag.kind.name(),
        severity: diag.severity,
        message: diag.message.clone(),
        help: diag.help.clone(),
        source,
        line: None,
        column: None,
        end_line: None,
        end_column: None,
    };

    match diag.span {
        // Same 1-indexed convention the LSP uses (see sqlsift-lsp diagnostics.rs).
        Some(span) if span.line > 0 => {
            let column = span.column.max(1);
            out.line = Some(span.line);
            out.column = Some(column);
            out.end_line = Some(span.line);
            out.end_column = Some(column + span.length.max(1));
        }
        // Parse errors carry no line info in their span, but sqlparser embeds
        // it in the message ("... at Line: 3, Column: 5").
        _ => {
            if let Some((line, column)) = parse_error_location(&diag.message) {
                out.line = Some(line);
                out.column = Some(column);
                out.end_line = Some(line);
                out.end_column = Some(column + 1);
            }
        }
    }

    out
}

/// 1-indexed `(line, column)` of the last non-whitespace character in `text`.
fn end_of_input(text: &str) -> (usize, usize) {
    let trimmed = text.trim_end();
    let line = trimmed.matches('\n').count() + 1;
    let last_line = trimmed.rsplit('\n').next().unwrap_or("");
    (line, last_line.chars().count().max(1))
}

/// Extract `(line, column)` from a sqlparser error message.
fn parse_error_location(message: &str) -> Option<(usize, usize)> {
    let idx = message.rfind("Line: ")?;
    let rest = &message[idx + "Line: ".len()..];
    let (line, rest) = rest.split_once(", Column: ")?;
    let column: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let line = line.trim().parse().ok()?;
    let column = column.parse().ok()?;
    (line > 0).then_some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);";

    #[test]
    fn valid_query_has_no_diagnostics() {
        let report = check_report(SCHEMA, "SELECT id, name FROM users;", "postgresql");
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(report.schema_warnings, 0);
    }

    #[test]
    fn typo_reports_location_and_help() {
        let report = check_report(SCHEMA, "SELECT naem FROM users;", "postgresql");
        assert_eq!(report.diagnostics.len(), 1);
        let d = &report.diagnostics[0];
        assert_eq!(d.code, "E0002");
        assert_eq!(d.source, Source::Query);
        assert_eq!(d.line, Some(1));
        assert_eq!(d.column, Some(8));
        assert_eq!(d.end_column, Some(12));
        assert!(d.help.as_deref().unwrap_or("").contains("name"));
    }

    #[test]
    fn inline_directive_suppresses() {
        let report = check_report(
            SCHEMA,
            "SELECT naem FROM users; -- sqlsift:disable E0002",
            "postgresql",
        );
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn query_parse_error_has_location() {
        let report = check_report(SCHEMA, "SELECT id\nFROM users WHERE", "postgresql");
        assert_eq!(report.diagnostics.len(), 1);
        let d = &report.diagnostics[0];
        assert_eq!(d.code, "E1000");
        assert_eq!(d.line, Some(2));
    }

    #[test]
    fn unparseable_schema_is_a_warning() {
        let schema = format!("{SCHEMA}\nCREATE TABLE broken (id INTEGER,,);");
        let report = check_report(&schema, "SELECT id FROM users;", "postgresql");
        assert_eq!(report.schema_warnings, 1);
        let d = &report.diagnostics[0];
        assert_eq!(d.source, Source::Schema);
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.line, Some(2));
    }

    #[test]
    fn json_shape() {
        let json = check_json(SCHEMA, "SELECT nope FROM users", "sqlite");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let d = &v["diagnostics"][0];
        assert_eq!(d["code"], "E0002");
        assert_eq!(d["severity"], "error");
        assert_eq!(d["source"], "query");
        assert_eq!(v["schema_warnings"], 0);
    }

    #[test]
    fn parse_error_location_extraction() {
        assert_eq!(
            parse_error_location("Expected: an expression, found: EOF at Line: 3, Column: 17"),
            Some((3, 17))
        );
        assert_eq!(parse_error_location("no location here"), None);
    }
}
