//! Output formatting

use sqlsift_core::{Diagnostic, Severity};

use crate::args::OutputFormat;

/// Output formatter for diagnostics
pub struct OutputFormatter {
    format: OutputFormat,
    file_name: String,
}

impl OutputFormatter {
    pub fn new(format: OutputFormat, file_name: String) -> Self {
        Self { format, file_name }
    }

    /// Print diagnostics in the configured format
    pub fn print_diagnostics(&self, diagnostics: &[Diagnostic], source: &str) {
        match self.format {
            OutputFormat::Human => self.print_human(diagnostics, source),
            OutputFormat::Json => self.print_json(diagnostics),
            OutputFormat::Sarif => self.print_sarif(diagnostics),
        }
    }

    fn print_human(&self, diagnostics: &[Diagnostic], source: &str) {
        for diag in diagnostics {
            let severity_str = match diag.severity {
                Severity::Error => "\x1b[31merror\x1b[0m",
                Severity::Warning => "\x1b[33mwarning\x1b[0m",
                Severity::Info => "\x1b[34minfo\x1b[0m",
            };

            // Print main message
            eprintln!("{}[{}]: {}", severity_str, diag.code(), diag.message);

            // Print file location if we have a span
            if let Some(span) = &diag.span {
                // Use line/column from span if available, otherwise compute from offset
                let (line, col) = if span.line > 0 {
                    (span.line, span.column)
                } else {
                    offset_to_line_col(source, span.offset)
                };
                eprintln!("  --> {}:{}:{}", self.file_name, line, col);

                // Print source line with annotation
                if let Some(source_line) = get_source_line(source, line) {
                    eprintln!("   |");
                    eprintln!("{:>3} | {}", line, source_line);

                    // Print caret annotation
                    let padding = " ".repeat(col.saturating_sub(1));
                    let underline = "^".repeat(
                        span.length
                            .min(source_line.len().saturating_sub(col) + 1)
                            .max(1),
                    );
                    eprintln!("   | {}{}", padding, underline);
                }
            }

            // Print help if available
            if let Some(help) = &diag.help {
                eprintln!("   = help: {}", help);
            }

            eprintln!();
        }
    }

    fn print_json(&self, diagnostics: &[Diagnostic]) {
        let output = serde_json::json!({
            "file": self.file_name,
            "diagnostics": diagnostics
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    fn print_sarif(&self, diagnostics: &[Diagnostic]) {
        let results: Vec<serde_json::Value> = diagnostics
            .iter()
            .map(|d| {
                let mut location = serde_json::json!({
                    "artifactLocation": {
                        "uri": self.file_name
                    }
                });

                // Add region if we have span information
                if let Some(span) = &d.span {
                    if span.line > 0 {
                        location["region"] = serde_json::json!({
                            "startLine": span.line,
                            "startColumn": span.column,
                            "endColumn": span.column + span.length
                        });
                    }
                }

                serde_json::json!({
                    "ruleId": d.code(),
                    "level": match d.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                        Severity::Info => "note",
                    },
                    "message": {
                        "text": d.message
                    },
                    "locations": [{
                        "physicalLocation": location
                    }]
                })
            })
            .collect();

        let sarif = serde_json::json!({
            "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
            "version": "2.1.0",
            "runs": [{
                "tool": {
                    "driver": {
                        "name": "sqlsift",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                },
                "results": results
            }]
        });

        println!("{}", serde_json::to_string_pretty(&sarif).unwrap());
    }
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

/// Get a specific line from source (1-indexed)
fn get_source_line(source: &str, line: usize) -> Option<&str> {
    source.lines().nth(line.saturating_sub(1))
}
