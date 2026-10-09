//! Inline comment directives for suppressing diagnostics
//!
//! Supports:
//! - `-- sqlsift:disable E0002` (same line: suppress on this line; standalone: suppress on next line)
//! - `-- sqlsift:disable E0002, E0003` (multiple rules)
//! - `-- sqlsift:disable column-not-found` (rule names work too)
//! - `-- sqlsift:disable` (suppress all rules)

use std::collections::{HashMap, HashSet};

use crate::error::DiagnosticKind;
use crate::rules::{find_rule, similar_rule_name};

/// Parsed inline disable directives from SQL comments
pub struct InlineDirectives {
    /// Map from line number (1-indexed) to disabled rule codes.
    /// `None` means all rules are disabled on that line.
    disabled_lines: HashMap<usize, Option<HashSet<String>>>,
}

impl InlineDirectives {
    /// Parse inline disable directives from SQL text
    pub fn parse(sql: &str) -> Self {
        let mut disabled_lines: HashMap<usize, Option<HashSet<String>>> = HashMap::new();
        let mut pending_codes: Option<Option<HashSet<String>>> = None;

        for (idx, line) in sql.lines().enumerate() {
            let line_num = idx + 1; // 1-indexed to match sqlparser Span
            let trimmed = line.trim();

            if let Some(codes) = parse_directive_from_line(line) {
                if trimmed.starts_with("--") {
                    // Standalone comment line: accumulate and apply to next SQL line
                    match &mut pending_codes {
                        Some(existing) => {
                            merge_codes(existing, codes);
                        }
                        None => {
                            pending_codes = Some(codes);
                        }
                    }
                } else {
                    // Inline comment (SQL + -- sqlsift:disable): applies to this line
                    merge_into_map(&mut disabled_lines, line_num, codes);
                }
            } else if pending_codes.is_some() && !trimmed.is_empty() && !trimmed.starts_with("--") {
                // Non-comment, non-empty line: apply pending disables
                let codes = pending_codes.take().unwrap();
                merge_into_map(&mut disabled_lines, line_num, codes);
            }
        }

        Self { disabled_lines }
    }

    /// Check if a diagnostic of the given kind on the given line should be suppressed
    pub fn is_suppressed(&self, kind: DiagnosticKind, line: usize) -> bool {
        self.suppresses(kind.code(), line) || self.suppresses(kind.name(), line)
    }

    /// Whether the rule code or name `id` is disabled on the given line
    fn suppresses(&self, id: &str, line: usize) -> bool {
        match self.disabled_lines.get(&line) {
            Some(None) => true, // All rules disabled
            Some(Some(ids)) => ids.iter().any(|i| i.eq_ignore_ascii_case(id)),
            None => false,
        }
    }

    /// Names in the disable directives for the given line that are no rule code or
    /// name (most likely misspelled), sorted
    pub fn unknown_ids(&self, line: usize) -> Vec<&str> {
        let Some(Some(ids)) = self.disabled_lines.get(&line) else {
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
                    "'{}' in the sqlsift:disable comment is not a rule. Did you mean '{}'?",
                    id, suggestion
                ),
                None => format!(
                    "'{}' in the sqlsift:disable comment is not a rule (run `sqlsift rules` to list rules)",
                    id
                ),
            })
            .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }
}

/// Parse a `-- sqlsift:disable ...` directive from a line.
/// Returns `Some(None)` for "disable all", `Some(Some(set))` for specific codes.
/// Returns `None` if no directive is found.
fn parse_directive_from_line(line: &str) -> Option<Option<HashSet<String>>> {
    // Find `--` that's not inside a string literal
    let comment_start = find_line_comment(line)?;
    let comment = &line[comment_start + 2..]; // skip "--"

    // Look for "sqlsift:disable"
    let trimmed = comment.trim();
    let rest = trimmed.strip_prefix("sqlsift:disable")?;

    if rest.is_empty() {
        // `-- sqlsift:disable` (no codes = disable all)
        return Some(None);
    }

    // Must be followed by whitespace or comma
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let codes: HashSet<String> = rest
        .split([',', ' '])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if codes.is_empty() {
        Some(None)
    } else {
        Some(Some(codes))
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
fn merge_into_map(
    map: &mut HashMap<usize, Option<HashSet<String>>>,
    line: usize,
    codes: Option<HashSet<String>>,
) {
    match map.get_mut(&line) {
        Some(existing) => {
            merge_codes(existing, codes);
        }
        None => {
            map.insert(line, codes);
        }
    }
}

/// Merge new codes into existing codes. `None` means "all rules disabled".
fn merge_codes(existing: &mut Option<HashSet<String>>, new: Option<HashSet<String>>) {
    match (existing.as_mut(), new) {
        (_, None) => {
            // New disables all → override
            *existing = None;
        }
        (None, _) => {
            // Already disabling all → keep as-is
        }
        (Some(existing_set), Some(new_set)) => {
            existing_set.extend(new_set);
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
}
