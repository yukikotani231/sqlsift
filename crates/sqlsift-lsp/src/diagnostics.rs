use std::collections::HashSet;

use tower_lsp::lsp_types::{self, NumberOrString, Position, Range};

use sqlsift_core::{Diagnostic, Severity, Span};

/// Convert sqlsift diagnostics to LSP diagnostics, filtering disabled rules
pub fn to_lsp_diagnostics(
    diagnostics: &[Diagnostic],
    disabled_rules: &HashSet<String>,
) -> Vec<lsp_types::Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| !disabled_rules.contains(d.code()))
        .map(to_lsp_diagnostic)
        .collect()
}

fn to_lsp_diagnostic(diag: &Diagnostic) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: span_to_range(diag.span.as_ref()),
        severity: Some(to_lsp_severity(diag.severity)),
        code: Some(NumberOrString::String(diag.code().to_string())),
        source: Some("sqlsift".to_string()),
        message: format_message(diag),
        ..Default::default()
    }
}

/// Convert Span (1-indexed) to LSP Range (0-indexed)
fn span_to_range(span: Option<&Span>) -> Range {
    match span {
        Some(s) if s.line > 0 => {
            let line = (s.line - 1) as u32;
            let col = s.column.saturating_sub(1) as u32;
            Range {
                start: Position::new(line, col),
                end: Position::new(line, col + s.length as u32),
            }
        }
        _ => Range::default(),
    }
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
        let range = span_to_range(Some(&span));
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.character, 5);
    }

    #[test]
    fn test_span_to_range_no_span() {
        let range = span_to_range(None);
        assert_eq!(range, Range::default());
    }

    #[test]
    fn test_span_to_range_zero_line_fallback() {
        let span = Span::new(0, 10);
        let range = span_to_range(Some(&span));
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

    #[test]
    fn test_disabled_rules_filtering() {
        let diagnostics = vec![
            Diagnostic::error(DiagnosticKind::TableNotFound, "Table 'a'"),
            Diagnostic::error(DiagnosticKind::ColumnNotFound, "Column 'b'"),
            Diagnostic::error(DiagnosticKind::TypeMismatch, "Type mismatch"),
        ];
        let disabled: HashSet<String> = ["E0001".to_string()].into();
        let result = to_lsp_diagnostics(&diagnostics, &disabled);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].code,
            Some(NumberOrString::String("E0002".to_string()))
        );
        assert_eq!(
            result[1].code,
            Some(NumberOrString::String("E0003".to_string()))
        );
    }
}
