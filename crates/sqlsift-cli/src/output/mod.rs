//! Output formatting

use std::io::{IsTerminal, Write};

use sqlsift_core::{Diagnostic, DiagnosticKind, Severity};

use crate::args::OutputFormat;

/// Documentation anchor for diagnostic rules (used as SARIF `helpUri`)
const RULES_HELP_URI: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "#diagnostic-rules");

/// Known rules with a short description, in SARIF `rules` order
const RULES: &[(DiagnosticKind, &str)] = &[
    (
        DiagnosticKind::TableNotFound,
        "Referenced table does not exist in schema",
    ),
    (
        DiagnosticKind::ColumnNotFound,
        "Referenced column does not exist in table",
    ),
    (
        DiagnosticKind::TypeMismatch,
        "Type incompatibility in expression",
    ),
    (
        DiagnosticKind::PotentialNullViolation,
        "Potential NOT NULL violation",
    ),
    (
        DiagnosticKind::ColumnCountMismatch,
        "INSERT column count doesn't match values",
    ),
    (
        DiagnosticKind::AmbiguousColumn,
        "Column reference is ambiguous across tables",
    ),
    (
        DiagnosticKind::JoinTypeMismatch,
        "JOIN condition compares incompatible types",
    ),
    (
        DiagnosticKind::MissingRequiredColumn,
        "INSERT omits a NOT NULL column without a default",
    ),
    (DiagnosticKind::ParseError, "SQL could not be parsed"),
];

/// Diagnostics produced for a single file
pub struct FileDiagnostics {
    pub file: String,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Output formatter for diagnostics
pub struct OutputFormatter {
    format: OutputFormat,
    color: bool,
}

/// Whether ANSI colors should be used for output written to stderr.
///
/// Colors are disabled when `NO_COLOR` is set (to any non-empty value) or
/// stderr is not a terminal.
pub fn use_color() -> bool {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    !no_color && std::io::stderr().is_terminal()
}

impl OutputFormatter {
    pub fn new(format: OutputFormat) -> Self {
        Self {
            format,
            color: use_color(),
        }
    }

    /// Print diagnostics for all files in the configured format.
    ///
    /// JSON and SARIF always produce exactly one document, even when there
    /// are no diagnostics. Files without diagnostics are omitted.
    pub fn print(&self, files: &[FileDiagnostics]) {
        let files: Vec<&FileDiagnostics> =
            files.iter().filter(|f| !f.diagnostics.is_empty()).collect();
        match self.format {
            OutputFormat::Human => {
                for f in files {
                    self.print_human(f);
                }
            }
            OutputFormat::Json => print_json(&files),
            OutputFormat::Sarif => print_sarif(&files),
        }
    }

    fn paint(&self, text: &str, ansi: &str) -> String {
        if self.color {
            format!("\x1b[{ansi}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Human-readable diagnostics on stderr. Written through one buffered, locked
    /// handle (stderr is unbuffered), and the source is split into lines once.
    fn print_human(&self, file: &FileDiagnostics) {
        let source = &file.source;
        let lines: Vec<&str> = source.lines().collect();
        let mut out = std::io::BufWriter::new(std::io::stderr().lock());
        // Diagnostics are best-effort output: ignore write errors (e.g. a closed pipe)
        let _ = self.write_human(&mut out, file, &lines);
    }

    fn write_human(
        &self,
        out: &mut impl Write,
        file: &FileDiagnostics,
        lines: &[&str],
    ) -> std::io::Result<()> {
        let source = &file.source;
        for diag in &file.diagnostics {
            let severity_str = match diag.severity {
                Severity::Error => self.paint("error", "31"),
                Severity::Warning => self.paint("warning", "33"),
                Severity::Info => self.paint("info", "34"),
            };

            // Print main message
            writeln!(out, "{}[{}]: {}", severity_str, diag.code(), diag.message)?;

            // Print file location if we have a span
            if let (Some(span), Some((line, col))) = (&diag.span, location(diag, source)) {
                writeln!(out, "  --> {}:{}:{}", file.file, line, col)?;

                // Print source line with annotation
                if let Some(source_line) = lines.get(line.saturating_sub(1)).copied() {
                    let width = line.to_string().len().max(3);
                    let gutter = " ".repeat(width);
                    writeln!(out, "{gutter} |")?;
                    writeln!(out, "{line:>width$} | {source_line}")?;

                    // Print caret annotation
                    let padding = " ".repeat(col.saturating_sub(1));
                    let underline = "^".repeat(
                        span.length
                            .min(source_line.len().saturating_sub(col) + 1)
                            .max(1),
                    );
                    writeln!(out, "{gutter} | {padding}{underline}")?;
                }
            }

            // Print help if available, aligned with the source gutter
            if let Some(help) = &diag.help {
                let width =
                    location(diag, source).map_or(3, |(line, _)| line.to_string().len().max(3));
                writeln!(out, "{} = help: {}", " ".repeat(width), help)?;
            }

            writeln!(out)?;
        }
        out.flush()
    }
}

/// Resolve the 1-indexed (line, column) of a diagnostic, if it has a span
fn location(diag: &Diagnostic, source: &str) -> Option<(usize, usize)> {
    let span = diag.span.as_ref()?;
    // Use line/column from span if available, otherwise compute from offset
    Some(if span.line > 0 {
        (span.line, span.column)
    } else {
        offset_to_line_col(source, span.offset)
    })
}

/// JSON output: `{"files": [{"file": ..., "diagnostics": [...]}, ...]}`
fn print_json(files: &[&FileDiagnostics]) {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            let diagnostics: Vec<serde_json::Value> = f
                .diagnostics
                .iter()
                .map(|d| {
                    let mut value = serde_json::to_value(d).unwrap_or_default();
                    let loc = location(d, &f.source);
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("code".into(), d.code().into());
                        obj.insert("line".into(), loc.map(|(l, _)| l).into());
                        obj.insert("column".into(), loc.map(|(_, c)| c).into());
                    }
                    value
                })
                .collect();
            serde_json::json!({
                "file": f.file,
                "diagnostics": diagnostics,
            })
        })
        .collect();
    let output = serde_json::json!({ "files": files });
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

/// SARIF 2.1.0 output with a single run containing results for all files
fn print_sarif(files: &[&FileDiagnostics]) {
    let mut rules: Vec<serde_json::Value> = RULES
        .iter()
        .map(|(kind, description)| sarif_rule(*kind, description))
        .collect();

    let mut results = Vec::new();
    for f in files {
        for d in &f.diagnostics {
            let rule_index = match rules.iter().position(|r| r["id"] == d.code()) {
                Some(i) => i,
                None => {
                    rules.push(sarif_rule(d.kind, d.kind.name()));
                    rules.len() - 1
                }
            };

            let mut physical = serde_json::json!({
                "artifactLocation": {
                    // SARIF URIs use `/` separators, also for Windows paths
                    "uri": f.file.replace('\\', "/")
                }
            });

            // Add region if we have span information
            if let (Some(span), Some((line, col))) = (&d.span, location(d, &f.source)) {
                physical["region"] = serde_json::json!({
                    "startLine": line,
                    "startColumn": col,
                    "endColumn": col + span.length
                });
            }

            results.push(serde_json::json!({
                "ruleId": d.code(),
                "ruleIndex": rule_index,
                "level": match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "note",
                },
                "message": {
                    "text": d.message
                },
                "locations": [{
                    "physicalLocation": physical
                }]
            }));
        }
    }

    let sarif = serde_json::json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "sqlsift",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": env!("CARGO_PKG_REPOSITORY"),
                    "rules": rules
                }
            },
            "results": results
        }]
    });

    println!("{}", serde_json::to_string_pretty(&sarif).unwrap());
}

fn sarif_rule(kind: DiagnosticKind, description: &str) -> serde_json::Value {
    serde_json::json!({
        "id": kind.code(),
        "name": kind.name(),
        "shortDescription": { "text": description },
        "helpUri": RULES_HELP_URI
    })
}

/// Convert byte offset to line and column (1-indexed)
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    (line, col)
}
