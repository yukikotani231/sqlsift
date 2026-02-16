//! Inline comment directives for suppressing diagnostics
//!
//! Supports:
//! - `-- sqlsift:disable E0002` (same line: suppress on this line; standalone: suppress on next line)
//! - `-- sqlsift:disable E0002, E0003` (multiple rules)
//! - `-- sqlsift:disable` (suppress all rules)

use std::collections::{HashMap, HashSet};

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

    /// Check if a diagnostic with the given code on the given line should be suppressed
    pub fn is_suppressed(&self, code: &str, line: usize) -> bool {
        match self.disabled_lines.get(&line) {
            Some(None) => true, // All rules disabled
            Some(Some(codes)) => codes.contains(code),
            None => false,
        }
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
        .map(|s| s.to_uppercase())
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
    fn test_inline_same_line() {
        let directives =
            InlineDirectives::parse("SELECT bad_col FROM users -- sqlsift:disable E0002");
        assert!(directives.is_suppressed("E0002", 1));
        assert!(!directives.is_suppressed("E0001", 1));
    }

    #[test]
    fn test_standalone_next_line() {
        let sql = "-- sqlsift:disable E0002\nSELECT bad_col FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0002", 2));
        assert!(!directives.is_suppressed("E0002", 1));
    }

    #[test]
    fn test_multiple_codes() {
        let sql = "SELECT * FROM t -- sqlsift:disable E0001, E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0001", 1));
        assert!(directives.is_suppressed("E0002", 1));
        assert!(!directives.is_suppressed("E0003", 1));
    }

    #[test]
    fn test_disable_all() {
        let sql = "SELECT * FROM t -- sqlsift:disable";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0001", 1));
        assert!(directives.is_suppressed("E0002", 1));
        assert!(directives.is_suppressed("E9999", 1));
    }

    #[test]
    fn test_standalone_disable_all_next_line() {
        let sql = "-- sqlsift:disable\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0001", 2));
        assert!(!directives.is_suppressed("E0001", 1));
    }

    #[test]
    fn test_multiple_standalone_directives_accumulate() {
        let sql = "-- sqlsift:disable E0001\n-- sqlsift:disable E0002\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0001", 3));
        assert!(directives.is_suppressed("E0002", 3));
        assert!(!directives.is_suppressed("E0003", 3));
    }

    #[test]
    fn test_no_directive() {
        let sql = "SELECT * FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.is_suppressed("E0001", 1));
    }

    #[test]
    fn test_directive_inside_string_ignored() {
        let sql = "SELECT '-- sqlsift:disable E0002' FROM users";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.is_suppressed("E0002", 1));
    }

    #[test]
    fn test_case_insensitive_codes() {
        let sql = "SELECT * FROM t -- sqlsift:disable e0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0002", 1));
    }

    #[test]
    fn test_skip_empty_lines_between_directive_and_sql() {
        let sql = "-- sqlsift:disable E0001\n\nSELECT * FROM t";
        let directives = InlineDirectives::parse(sql);
        // Empty line doesn't consume the pending directive
        assert!(directives.is_suppressed("E0001", 3));
    }

    #[test]
    fn test_comma_separated_no_spaces() {
        let sql = "SELECT * FROM t -- sqlsift:disable E0001,E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0001", 1));
        assert!(directives.is_suppressed("E0002", 1));
    }

    #[test]
    fn test_not_a_directive() {
        let sql = "SELECT * FROM t -- sqlsift:disabled E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(!directives.is_suppressed("E0002", 1));
    }

    #[test]
    fn test_double_quoted_identifier_with_dashes() {
        let sql = "SELECT \"col--name\" FROM t -- sqlsift:disable E0002";
        let directives = InlineDirectives::parse(sql);
        assert!(directives.is_suppressed("E0002", 1));
    }
}
