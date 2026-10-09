//! Inline comment directives for suppressing diagnostics
//!
//! Supports:
//! - `-- sqlsift:disable E0002` (same line: suppress on this line; standalone: suppress on next line)
//! - `-- sqlsift:disable E0002, E0003` (multiple rules)
//! - `-- sqlsift:disable column-not-found` (rule names work too)
//! - `-- sqlsift:disable` (suppress all rules)
//! - `-- sqlsift:disable-file E0002, ambiguous-column` (suppress rules in the whole file)
//! - `-- sqlsift:disable-file` (suppress all rules in the whole file)
//!
//! A `disable-file` directive may appear on any line of the file, as a standalone
//! comment or after SQL, and applies to every line (before and after it).

use std::collections::{HashMap, HashSet};

use crate::error::DiagnosticKind;
use crate::rules::{find_rule, similar_rule_name};

/// Rules disabled by a directive
#[derive(Debug)]
enum Disabled {
    /// A directive without rule names
    All,
    /// Rule codes or names, as written
    Rules(HashSet<String>),
}

impl Disabled {
    fn merge(&mut self, other: Disabled) {
        match (self, other) {
            (Disabled::All, _) => {}
            (this, Disabled::All) => *this = Disabled::All,
            (Disabled::Rules(ids), Disabled::Rules(new)) => ids.extend(new),
        }
    }

    /// Whether the rule code or name `id` is disabled
    fn contains(&self, id: &str) -> bool {
        match self {
            Disabled::All => true,
            Disabled::Rules(ids) => ids.iter().any(|i| i.eq_ignore_ascii_case(id)),
        }
    }
}

/// Parsed inline disable directives from SQL comments
pub struct InlineDirectives {
    /// Map from line number (1-indexed) to the rules disabled on that line
    disabled_lines: HashMap<usize, Disabled>,
    /// Rules disabled for the whole file by `-- sqlsift:disable-file`
    disabled_file: Option<Disabled>,
}

/// The kind of a `sqlsift:` comment directive
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectiveScope {
    /// `sqlsift:disable`: this line (inline) or the next SQL line (standalone)
    Line,
    /// `sqlsift:disable-file`: the whole file
    File,
}

impl InlineDirectives {
    /// Parse inline disable directives from SQL text
    pub fn parse(sql: &str) -> Self {
        let mut disabled_lines: HashMap<usize, Disabled> = HashMap::new();
        let mut pending_codes: Option<Disabled> = None;
        let mut disabled_file: Option<Disabled> = None;

        for (idx, line) in sql.lines().enumerate() {
            let line_num = idx + 1; // 1-indexed to match sqlparser Span
            let trimmed = line.trim();

            let directive = parse_directive_from_line(line);
            if let Some((DirectiveScope::File, codes)) = directive {
                match &mut disabled_file {
                    Some(existing) => existing.merge(codes),
                    None => disabled_file = Some(codes),
                }
                // Inline after SQL: the line still consumes a pending line directive
                if !trimmed.starts_with("--") {
                    if let Some(codes) = pending_codes.take() {
                        merge_into_map(&mut disabled_lines, line_num, codes);
                    }
                }
            } else if let Some((DirectiveScope::Line, codes)) = directive {
                if trimmed.starts_with("--") {
                    // Standalone comment line: accumulate and apply to next SQL line
                    match &mut pending_codes {
                        Some(existing) => existing.merge(codes),
                        None => pending_codes = Some(codes),
                    }
                } else {
                    // Inline comment (SQL + -- sqlsift:disable): applies to this line
                    merge_into_map(&mut disabled_lines, line_num, codes);
                }
            } else if !trimmed.trim_matches(';').trim().is_empty() && !trimmed.starts_with("--") {
                // Non-comment, non-empty line: apply pending disables (a line of only
                // `;`, such as a template literal's opening backtick in embedded SQL,
                // is not the line a directive is meant for)
                if let Some(codes) = pending_codes.take() {
                    merge_into_map(&mut disabled_lines, line_num, codes);
                }
            }
        }

        Self {
            disabled_lines,
            disabled_file,
        }
    }

    /// Check if a diagnostic of the given kind on the given line should be suppressed
    /// (by a line directive or a file directive)
    pub fn is_suppressed(&self, kind: DiagnosticKind, line: usize) -> bool {
        self.is_suppressed_in_file(kind)
            || self.suppresses(kind.code(), line)
            || self.suppresses(kind.name(), line)
    }

    /// Check if a diagnostic of the given kind is suppressed for the whole file
    /// (by `-- sqlsift:disable-file`), regardless of where it is reported
    pub fn is_suppressed_in_file(&self, kind: DiagnosticKind) -> bool {
        self.disabled_file
            .as_ref()
            .is_some_and(|d| d.contains(kind.code()) || d.contains(kind.name()))
    }

    /// Whether the rule code or name `id` is disabled on the given line
    fn suppresses(&self, id: &str, line: usize) -> bool {
        self.disabled_lines
            .get(&line)
            .is_some_and(|d| d.contains(id))
    }

    /// Names in the disable directives for the given line that are no rule code or
    /// name (most likely misspelled), sorted
    pub fn unknown_ids(&self, line: usize) -> Vec<&str> {
        let Some(Disabled::Rules(ids)) = self.disabled_lines.get(&line) else {
            return Vec::new();
        };
        let mut unknown: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| find_rule(id).is_none())
            .collect();
        unknown.sort_unstable();
        unknown
    }

    /// Explain why a diagnostic on `line` was not suppressed when that line's disable
    /// directive names an unknown rule: "Did you mean ...?"
    pub fn unknown_id_help(&self, line: usize) -> Option<String> {
        let notes: Vec<String> = self
            .unknown_ids(line)
            .into_iter()
            .map(|id| match similar_rule_name(id) {
                Some(suggestion) => format!(
                    "'{id}' in the sqlsift:disable comment is not a rule. Did you mean '{suggestion}'?"
                ),
                None => format!(
                    "'{id}' in the sqlsift:disable comment is not a rule (run `sqlsift rules` to list rules)"
                ),
            })
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }
}

/// Parse a `-- sqlsift:disable ...` or `-- sqlsift:disable-file ...` directive from a line.
/// Returns the directive's scope and the rules it disables, or `None` if no directive
/// is found.
fn parse_directive_from_line(line: &str) -> Option<(DirectiveScope, Disabled)> {
    // Find `--` that's not inside a string literal
    let comment_start = find_line_comment(line)?;
    let comment = &line[comment_start + 2..]; // skip "--"

    // Look for "sqlsift:disable"
    let trimmed = comment.trim();
    let rest = trimmed.strip_prefix("sqlsift:disable")?;
    let (scope, rest) = match rest.strip_prefix("-file") {
        Some(rest) => (DirectiveScope::File, rest),
        None => (DirectiveScope::Line, rest),
    };

    if rest.is_empty() {
        // `-- sqlsift:disable` (no codes = disable all)
        return Some((scope, Disabled::All));
    }

    // Must be followed by whitespace or comma
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let codes: HashSet<String> = rest
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if codes.is_empty() {
        Some((scope, Disabled::All))
    } else {
        Some((scope, Disabled::Rules(codes)))
    }
}

/// Find the byte offset of `--` that starts a line comment (not inside a string).
fn find_line_comment(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'\'' => {
                // Skip single-quoted string
                i += 1;
                while i < len {
                    if bytes[i] == b'\'' {
                        i += 1;
                        if i < len && bytes[i] == b'\'' {
                            i += 1; // escaped quote
                        } else {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'"' => {
                // Skip double-quoted identifier
                i += 1;
                while i < len && bytes[i] != b'"' {
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
            }
            b'-' if i + 1 < len && bytes[i + 1] == b'-' => {
                return Some(i);
            }
            _ => {
                i += 1;
            }
        }
    }

    None
}

/// Merge new codes into an existing entry in the map
fn merge_into_map(map: &mut HashMap<usize, Disabled>, line: usize, codes: Disabled) {
    match map.get_mut(&line) {
        Some(existing) => existing.merge(codes),
        None => {
            map.insert(line, codes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_rule_names_get_a_suggestion() {
        let sql = "-- sqlsift:disable ambigous-column, E0002, zzz\nSELECT 1";
        let directives = InlineDirectives::parse(sql);
        assert_eq!(directives.unknown_ids(2), vec!["ambigous-column", "zzz"]);
        assert_eq!(
            directives.unknown_id_help(2).as_deref(),
            Some(
                "'ambigous-column' in the sqlsift:disable comment is not a rule. \
                 Did you mean 'ambiguous-column'?\n\
                 'zzz' in the sqlsift:disable comment is not a rule \
                 (run `sqlsift rules` to list rules)"
            )
        );
        assert_eq!(directives.unknown_id_help(1), None);
    }

    #[test]
    fn test_inline_same_line() {
        let directives =
            InlineDirectives::parse("SELECT bad_col FROM users -- sqlsift:disable E0002");
        assert!(directives.suppresses("E0002", 1));
        assert!(!directives.suppresses("E0001", 1));
    }

    #[test]
    fn test_standalone_next_line() {
        let sql = "-- sqlsift:disable E0002\nSELECT bad_col FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0002", 2));
        assert!(!directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_multiple_codes() {
        let sql = "SELECT * FROM t -- sqlsift:disable E0001, E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0001", 1));
        assert!(directives.suppresses("E0002", 1));
        assert!(!directives.suppresses("E0003", 1));
    }

    #[test]
    fn test_disable_all() {
        let sql = "SELECT * FROM t -- sqlsift:disable";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0001", 1));
        assert!(directives.suppresses("E0002", 1));
        assert!(directives.suppresses("E9999", 1));
    }

    #[test]
    fn test_standalone_disable_all_next_line() {
        let sql = "-- sqlsift:disable\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0001", 2));
        assert!(!directives.suppresses("E0001", 1));
    }

    #[test]
    fn test_multiple_standalone_directives_accumulate() {
        let sql = "-- sqlsift:disable E0001\n-- sqlsift:disable E0002\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0001", 3));
        assert!(directives.suppresses("E0002", 3));
        assert!(!directives.suppresses("E0003", 3));
    }

    #[test]
    fn test_no_directive() {
        let sql = "SELECT * FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.suppresses("E0001", 1));
    }

    #[test]
    fn test_directive_inside_string_ignored() {
        let sql = "SELECT '-- sqlsift:disable E0002' FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_case_insensitive_codes() {
        let sql = "SELECT * FROM t -- sqlsift:disable e0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_skip_empty_lines_between_directive_and_sql() {
        let sql = "-- sqlsift:disable E0001\n\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        // Empty line doesn't consume the pending directive
        assert!(directives.suppresses("E0001", 3));
    }

    #[test]
    fn test_comma_separated_no_spaces() {
        let sql = "SELECT * FROM t -- sqlsift:disable E0001,E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0001", 1));
        assert!(directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_not_a_directive() {
        let sql = "SELECT * FROM t -- sqlsift:disabled E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_double_quoted_identifier_with_dashes() {
        let sql = "SELECT \"col--name\" FROM t -- sqlsift:disable E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.suppresses("E0002", 1));
    }

    #[test]
    fn test_rule_names_suppress_like_codes() {
        let directives =
            InlineDirectives::parse("SELECT nme FROM users; -- sqlsift:disable column-not-found");
        assert!(directives.is_suppressed(DiagnosticKind::ColumnNotFound, 1));
        assert!(!directives.is_suppressed(DiagnosticKind::TableNotFound, 1));
        let directives = InlineDirectives::parse("SELECT nme FROM users; -- sqlsift:disable E0002");
        assert!(directives.is_suppressed(DiagnosticKind::ColumnNotFound, 1));
    }

    #[test]
    fn test_disable_file_specific_rules_anywhere() {
        let sql = "SELECT nme FROM users;\nSELECT 1;\n-- sqlsift:disable-file E0002, ambiguous-column\nSELECT x FROM t;";
        let directives = InlineDirectives::parse(sql);
        // Applies to lines before and after the directive
        for line in 1..=4 {
            assert!(directives.is_suppressed(DiagnosticKind::ColumnNotFound, line));
            assert!(directives.is_suppressed(DiagnosticKind::AmbiguousColumn, line));
            assert!(!directives.is_suppressed(DiagnosticKind::TableNotFound, line));
        }
        assert!(directives.is_suppressed_in_file(DiagnosticKind::ColumnNotFound));
        assert!(!directives.is_suppressed_in_file(DiagnosticKind::TableNotFound));
    }

    #[test]
    fn test_disable_file_all_rules() {
        let directives = InlineDirectives::parse("-- sqlsift:disable-file\nSELECT x FROM t;");
        assert!(directives.is_suppressed_in_file(DiagnosticKind::TableNotFound));
        assert!(directives.is_suppressed_in_file(DiagnosticKind::ParseError));
        assert!(directives.is_suppressed(DiagnosticKind::ColumnNotFound, 99));
    }

    #[test]
    fn test_disable_file_is_not_a_line_directive() {
        // A file directive must not also act as a next-line directive...
        let directives = InlineDirectives::parse("-- sqlsift:disable-file E0001\nSELECT x FROM t;");
        assert!(!directives.suppresses("E0002", 2));
        assert!(!directives.is_suppressed(DiagnosticKind::ColumnNotFound, 2));
        // ...and a line directive must not disable rules for the whole file
        let directives =
            InlineDirectives::parse("-- sqlsift:disable E0002\nSELECT x FROM t;\nSELECT y FROM t;");
        assert!(!directives.is_suppressed_in_file(DiagnosticKind::ColumnNotFound));
        assert!(!directives.is_suppressed(DiagnosticKind::ColumnNotFound, 3));
    }

    #[test]
    fn test_disable_file_lookalikes_are_ignored() {
        for sql in [
            "SELECT 1; -- sqlsift:disable-files E0002",
            "SELECT 1; -- sqlsift:disable-fileE0002",
            "SELECT '-- sqlsift:disable-file' FROM t",
        ] {
            let directives = InlineDirectives::parse(sql);
            assert!(
                !directives.is_suppressed_in_file(DiagnosticKind::ColumnNotFound),
                "{sql}"
            );
            assert!(
                !directives.is_suppressed(DiagnosticKind::ColumnNotFound, 1),
                "{sql}"
            );
        }
    }

    #[test]
    fn test_disable_file_inline_after_sql_and_merging() {
        let sql = "-- sqlsift:disable E0003\nSELECT 1; -- sqlsift:disable-file e0001\n-- sqlsift:disable-file column-not-found";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed_in_file(DiagnosticKind::TableNotFound));
        assert!(directives.is_suppressed_in_file(DiagnosticKind::ColumnNotFound));
        assert!(!directives.is_suppressed_in_file(DiagnosticKind::TypeMismatch));
        // The pending line directive still applies to the SQL on line 2
        assert!(directives.is_suppressed(DiagnosticKind::TypeMismatch, 2));
        assert!(!directives.is_suppressed(DiagnosticKind::TypeMismatch, 3));
    }
}
