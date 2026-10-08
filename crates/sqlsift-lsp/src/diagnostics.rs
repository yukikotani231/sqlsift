use tower_lsp::lsp_types::{self, NumberOrString, Position, Range};

use sqlsift_core::{Diagnostic, Severity, Span};

/// Convert sqlsift diagnostics to LSP diagnostics. (Rule levels are applied by the
/// analyzer: diagnostics of rules that are off never get here.)
///
/// `text` is the analyzed document, used to convert character columns into
/// the UTF-16 code unit offsets that LSP positions use.
pub fn to_lsp_diagnostics(diagnostics: &[Diagnostic], text: &str) -> Vec<lsp_types::Diagnostic> {
    diagnostics
        .iter()
        .map(|d| to_lsp_diagnostic(d, text))
        .collect()
}

fn to_lsp_diagnostic(diag: &Diagnostic, text: &str) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: span_to_range(diag.span.as_ref(), text),
        severity: Some(to_lsp_severity(diag.severity)),
        code: Some(NumberOrString::String(diag.code().to_string())),
        source: Some("sqlsift".to_string()),
        message: format_message(diag),
        ..Default::default()
    }
}

/// Convert Span (1-indexed, character columns) to LSP Range (0-indexed, UTF-16 columns)
fn span_to_range(span: Option<&Span>, text: &str) -> Range {
    match span {
        Some(s) if s.line > 0 => {
            let line_text = text.lines().nth(s.line - 1).unwrap_or("");
            let start_chars = s.column.saturating_sub(1);
            let line = (s.line - 1) as u32;
            Range {
                start: Position::new(line, utf16_offset(line_text, start_chars)),
                end: Position::new(line, utf16_offset(line_text, start_chars + s.length)),
            }
        }
        _ => Range::default(),
    }
}

/// Number of UTF-16 code units in the first `chars` characters of `line`.
/// Characters past the end of the line count as one unit each.
fn utf16_offset(line: &str, chars: usize) -> u32 {
    let mut units = 0;
    let mut count = 0;
    for c in line.chars().take(chars) {
        units += c.len_utf16();
        count += 1;
    }
    (units + chars.saturating_sub(count)) as u32
}

fn to_lsp_severity(severity: Severity) -> lsp_types::DiagnosticSeverity {
    match severity {
        Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
        Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
        Severity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
    }
}

fn format_message(diag: &Diagnostic) -> String {
    match &diag.help {
        Some(help) => format!("{}\n\nHelp: {}", diag.message, help),
        None => diag.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlsift_core::DiagnosticKind;

    #[test]
    fn test_span_to_range_1indexed_to_0indexed() {
        let span = Span::with_location(1, 1, 5);
        let range = span_to_range(Some(&span), "");
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.character, 5);
    }

    #[test]
    fn test_span_to_range_utf16() {
        // 'é' is 1 UTF-16 unit, U+1F600 is 2
        let text = "x\nSELECT '\u{1F600}é', nme";
        let span = Span::with_location(2, 14, 3);
        let range = span_to_range(Some(&span), text);
        assert_eq!(range.start, Position::new(1, 14));
        assert_eq!(range.end, Position::new(1, 17));
    }

    #[test]
    fn test_span_to_range_no_span() {
        let range = span_to_range(None, "");
        assert_eq!(range, Range::default());
    }

    #[test]
    fn test_span_to_range_zero_line_fallback() {
        let span = Span::new(0, 10);
        let range = span_to_range(Some(&span), "");
        assert_eq!(range, Range::default());
    }

    #[test]
    fn test_severity_mapping() {
        assert_eq!(
            to_lsp_severity(Severity::Error),
            lsp_types::DiagnosticSeverity::ERROR
        );
        assert_eq!(
            to_lsp_severity(Severity::Warning),
            lsp_types::DiagnosticSeverity::WARNING
        );
        assert_eq!(
            to_lsp_severity(Severity::Info),
            lsp_types::DiagnosticSeverity::INFORMATION
        );
    }

    #[test]
    fn test_format_message_with_help() {
        let diag = Diagnostic::error(DiagnosticKind::TableNotFound, "Table 'foo' not found")
            .with_help("Did you mean 'bar'?");
        let msg = format_message(&diag);
        assert_eq!(msg, "Table 'foo' not found\n\nHelp: Did you mean 'bar'?");
    }

    #[test]
    fn test_format_message_without_help() {
        let diag = Diagnostic::error(DiagnosticKind::TableNotFound, "Table 'foo' not found");
        let msg = format_message(&diag);
        assert_eq!(msg, "Table 'foo' not found");
    }
}
