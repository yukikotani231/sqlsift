//! Baselines: record the diagnostics a project already has, so that only new
//! ones are reported (`sqlsift check --write-baseline`, `baseline` in `sqlsift.toml`)
//!
//! A diagnostic is identified by a [`Fingerprint`]:
//!
//! - the file, relative to the baseline file's directory with `/` separators
//!   (see [`file_key`])
//! - the rule code
//! - a hash of the text of the statement containing the diagnostic, with
//!   comments removed and whitespace collapsed, so that editing other
//!   statements, adding lines above it or reformatting it keeps the match
//! - the occurrence index among the file's diagnostics with the same code and
//!   statement hash (the same mistake made twice in one statement)
//!
//! Line numbers and messages are stored for people reading the file, but are
//! not used for matching.
//!
//! ```
//! use sqlsift_core::baseline::{Baseline, BaselineFilter};
//! use sqlsift_core::schema::SchemaBuilder;
//! use sqlsift_core::Analyzer;
//!
//! let mut builder = SchemaBuilder::new();
//! builder.parse("CREATE TABLE users (id INTEGER);").unwrap();
//! let (catalog, _) = builder.build();
//! let analyze = |sql: &str| Analyzer::new(&catalog).analyze(sql);
//!
//! // Record the existing problem
//! let old = "SELECT nme FROM users;";
//! let mut baseline = Baseline::default();
//! baseline.add_file("q.sql", old, &analyze(old));
//!
//! // Lines added above it don't matter; a new problem is still reported
//! let new = "SELECT id FROM users;\n\nSELECT nme FROM users;\nSELECT x FROM users;";
//! let filtered = BaselineFilter::new(&baseline).filter("q.sql", new, analyze(new));
//! assert_eq!(filtered.kept.len(), 1);
//! assert!(filtered.kept[0].message.contains("'x'"));
//! assert_eq!(filtered.suppressed.len(), 1);
//! ```

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Diagnostic;

/// File name used when no baseline path is configured
pub const DEFAULT_BASELINE_FILE: &str = "sqlsift-baseline.json";

/// Version of the baseline file format
pub const BASELINE_VERSION: u32 = 1;

/// A baseline file: the diagnostics that are known and not reported
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    /// File format version ([`BASELINE_VERSION`])
    pub version: u32,
    /// Known diagnostics
    #[serde(default)]
    pub entries: Vec<BaselineEntry>,
}

impl Default for Baseline {
    fn default() -> Self {
        Self {
            version: BASELINE_VERSION,
            entries: Vec::new(),
        }
    }
}

/// One known diagnostic
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineEntry {
    /// File path relative to the baseline file's directory, with `/` separators
    pub file: String,
    /// Rule code (e.g. `E0002`)
    pub code: String,
    /// Hash of the normalized statement text (16 hex digits; empty when the
    /// diagnostic has no location)
    pub statement_hash: String,
    /// Index among the file's diagnostics with the same code and statement hash
    #[serde(default)]
    pub occurrence: usize,
    /// Line of the diagnostic when the baseline was written (informational)
    #[serde(default)]
    pub line: usize,
    /// Message of the diagnostic (informational)
    #[serde(default)]
    pub message: String,
}

impl BaselineEntry {
    /// The key this entry is matched by
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            file: self.file.clone(),
            code: self.code.clone(),
            statement_hash: self.statement_hash.clone(),
            occurrence: self.occurrence,
        }
    }
}

/// What identifies a diagnostic in a baseline
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint {
    pub file: String,
    pub code: String,
    pub statement_hash: String,
    pub occurrence: usize,
}

impl Baseline {
    /// Parse a baseline file
    pub fn from_json(json: &str) -> Result<Self, String> {
        let baseline: Self =
            serde_json::from_str(json).map_err(|e| format!("invalid baseline file: {e}"))?;
        if baseline.version != BASELINE_VERSION {
            return Err(format!(
                "unsupported baseline version {} (expected {BASELINE_VERSION}); re-create it with --write-baseline",
                baseline.version
            ));
        }
        Ok(baseline)
    }

    /// Serialize as pretty-printed JSON (with a trailing newline), entries
    /// sorted so that the file diffs well
    pub fn to_json(&self) -> String {
        let mut sorted = self.clone();
        sorted.entries.sort_by(|a, b| {
            (&a.file, a.line, &a.code, a.occurrence, &a.statement_hash).cmp(&(
                &b.file,
                b.line,
                &b.code,
                b.occurrence,
                &b.statement_hash,
            ))
        });
        let mut json = serde_json::to_string_pretty(&sorted).unwrap_or_default();
        json.push('\n');
        json
    }

    /// Add the diagnostics of a file. `file` is its [`file_key`]; `source` is
    /// the text the diagnostics were reported on.
    pub fn add_file(&mut self, file: &str, source: &str, diagnostics: &[Diagnostic]) {
        let fingerprints = fingerprints(file, source, diagnostics);
        for (diagnostic, fingerprint) in diagnostics.iter().zip(fingerprints) {
            self.entries.push(BaselineEntry {
                file: fingerprint.file,
                code: fingerprint.code,
                statement_hash: fingerprint.statement_hash,
                occurrence: fingerprint.occurrence,
                line: diagnostic.span.map_or(0, |s| s.line),
                message: diagnostic.message.clone(),
            });
        }
    }
}

/// Diagnostics of a file after removing the baselined ones
#[derive(Debug, Default)]
pub struct Filtered {
    /// Diagnostics not in the baseline
    pub kept: Vec<Diagnostic>,
    /// Baseline entries that matched a diagnostic
    pub suppressed: Vec<Fingerprint>,
}

/// Removes baselined diagnostics
#[derive(Debug, Clone, Default)]
pub struct BaselineFilter {
    known: HashSet<Fingerprint>,
}

impl BaselineFilter {
    pub fn new(baseline: &Baseline) -> Self {
        Self {
            known: baseline
                .entries
                .iter()
                .map(BaselineEntry::fingerprint)
                .collect(),
        }
    }

    /// Number of distinct entries
    pub fn len(&self) -> usize {
        self.known.len()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }

    /// Split the diagnostics of a file into new and baselined ones. `file` is
    /// its [`file_key`]; `source` is the text the diagnostics were reported on.
    pub fn filter(&self, file: &str, source: &str, diagnostics: Vec<Diagnostic>) -> Filtered {
        let fingerprints = fingerprints(file, source, &diagnostics);
        let mut filtered = Filtered::default();
        for (diagnostic, fingerprint) in diagnostics.into_iter().zip(fingerprints) {
            if self.known.contains(&fingerprint) {
                filtered.suppressed.push(fingerprint);
            } else {
                filtered.kept.push(diagnostic);
            }
        }
        filtered
    }

    /// Number of entries for the `checked` files that matched no diagnostic
    /// (problems that were fixed since the baseline was written)
    pub fn stale(&self, checked: &HashSet<String>, matched: &HashSet<Fingerprint>) -> usize {
        self.known
            .iter()
            .filter(|f| checked.contains(&f.file) && !matched.contains(*f))
            .count()
    }
}

/// The name of `path` in a baseline: relative to `base_dir` (the directory
/// containing the baseline file), with `/` separators, so that it is the same
/// whatever directory sqlsift runs in (and in the editor). Relative paths are
/// taken relative to the current directory. A path that shares no root with
/// `base_dir` (another drive on Windows) is kept as given.
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::baseline::file_key;
///
/// let base = Path::new("/project/ci");
/// assert_eq!(file_key(Path::new("/project/ci/./q.sql"), base), "q.sql");
/// assert_eq!(file_key(Path::new("/project/sql/q.sql"), base), "../sql/q.sql");
/// ```
pub fn file_key(path: &Path, base_dir: &Path) -> String {
    let absolute = crate::ignore::normalize(path);
    let base = crate::ignore::normalize(base_dir);
    let path_parts: Vec<_> = absolute.components().collect();
    let base_parts: Vec<_> = base.components().collect();
    let common = path_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return normalize_separators(&path.to_string_lossy());
    }
    let parts: Vec<String> = std::iter::repeat("..".to_string())
        .take(base_parts.len() - common)
        .chain(
            path_parts[common..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        )
        .collect();
    parts.join("/")
}

/// Use `/` separators and drop a leading `./`
fn normalize_separators(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut path = path.as_str();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path.to_string()
}

/// Fingerprints of `diagnostics` (in the same order), all in `file`
pub fn fingerprints(file: &str, source: &str, diagnostics: &[Diagnostic]) -> Vec<Fingerprint> {
    let file = normalize_separators(file);
    let ranges = statement_ranges(source);
    let lines = line_starts(source);
    let mut hashes: HashMap<Range<usize>, String> = HashMap::new();
    let mut counts: HashMap<(&'static str, String), usize> = HashMap::new();
    diagnostics
        .iter()
        .map(|diagnostic| {
            let statement_hash = diagnostic
                .span
                .map(|span| {
                    let offset = if span.line > 0 {
                        byte_offset(source, &lines, span.line, span.column)
                    } else {
                        span.offset.min(source.len())
                    };
                    let range = enclosing(&ranges, offset);
                    hashes
                        .entry(range.clone())
                        .or_insert_with(|| statement_hash(&source[range]))
                        .clone()
                })
                .unwrap_or_default();
            let code = diagnostic.code();
            let count = counts.entry((code, statement_hash.clone())).or_insert(0);
            let occurrence = *count;
            *count += 1;
            Fingerprint {
                file: file.clone(),
                code: code.to_string(),
                statement_hash,
                occurrence,
            }
        })
        .collect()
}

/// Hash of a statement's text with comments removed and whitespace collapsed
/// (FNV-1a, 64 bit, as 16 hex digits). Stable across platforms and versions.
///
/// ```
/// use sqlsift_core::baseline::statement_hash;
///
/// assert_eq!(
///     statement_hash("SELECT a\n  FROM t; -- note"),
///     statement_hash("-- a comment\nSELECT a FROM t"),
/// );
/// assert_ne!(statement_hash("SELECT a FROM t"), statement_hash("SELECT b FROM t"));
/// ```
pub fn statement_hash(statement: &str) -> String {
    format!(
        "{:016x}",
        fnv1a_64(normalize_statement(statement).as_bytes())
    )
}

/// FNV-1a, 64 bit
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

/// A statement's text without comments, runs of whitespace (outside quotes)
/// collapsed to one space, and without the trailing `;`
fn normalize_statement(statement: &str) -> String {
    let mut out = String::with_capacity(statement.len());
    let mut pending_space = false;
    for token in tokens(statement) {
        match token.kind {
            TokenKind::Whitespace | TokenKind::Comment => pending_space = true,
            TokenKind::Semicolon => {}
            TokenKind::Text => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push_str(&statement[token.range]);
            }
        }
    }
    out
}

/// Byte ranges of the `;`-separated statements of `source` (each range ends
/// after its `;`; the last one runs to the end of the source)
fn statement_ranges(source: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for token in tokens(source) {
        if token.kind == TokenKind::Semicolon {
            ranges.push(start..token.range.end);
            start = token.range.end;
        }
    }
    if start < source.len() || ranges.is_empty() {
        ranges.push(start..source.len());
    }
    ranges
}

/// The range containing `offset` (an offset past the end belongs to the last)
fn enclosing(ranges: &[Range<usize>], offset: usize) -> Range<usize> {
    let i = ranges.partition_point(|r| r.end <= offset);
    ranges
        .get(i)
        .or_else(|| ranges.last())
        .cloned()
        .unwrap_or(0..0)
}

/// Byte offsets of the line starts
fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Byte offset of a 1-indexed line and character column
fn byte_offset(source: &str, lines: &[usize], line: usize, column: usize) -> usize {
    let Some(&start) = lines.get(line - 1) else {
        return source.len();
    };
    source[start..]
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(source.len(), |(i, _)| start + i)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Whitespace,
    Comment,
    Semicolon,
    /// Anything else, including quoted strings and identifiers
    Text,
}

#[derive(Debug)]
struct Token {
    kind: TokenKind,
    range: Range<usize>,
}

/// Split SQL into whitespace, comments, `;` and other text. Dialect neutral and
/// deliberately simple: quotes (`'`, `"`, `` ` ``, with doubled quotes as
/// escapes), dollar-quoted strings and `--` / `/* */` comments are
/// recognized, so that a `;` or `--` inside them doesn't split the statement.
fn tokens(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let kind = if b.is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            TokenKind::Whitespace
        } else if b == b';' {
            i += 1;
            TokenKind::Semicolon
        } else if sql[i..].starts_with("--") {
            i = sql[i..].find('\n').map_or(bytes.len(), |n| i + n);
            TokenKind::Comment
        } else if sql[i..].starts_with("/*") {
            i = sql[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |n| i + 2 + n + 2);
            TokenKind::Comment
        } else if matches!(b, b'\'' | b'"' | b'`') {
            i = end_of_quoted(bytes, i);
            TokenKind::Text
        } else if let Some(end) = end_of_dollar_quoted(sql, i) {
            i = end;
            TokenKind::Text
        } else {
            // A run of other characters (stopping before anything special)
            i += 1;
            while i < bytes.len() {
                let c = bytes[i];
                if c.is_ascii_whitespace()
                    || matches!(c, b';' | b'\'' | b'"' | b'`' | b'$')
                    || sql[i..].starts_with("--")
                    || sql[i..].starts_with("/*")
                {
                    break;
                }
                i += 1;
            }
            TokenKind::Text
        };
        tokens.push(Token {
            kind,
            range: start..i,
        });
    }
    tokens
}

/// End of the quoted text starting at `start` (a doubled quote is an escape)
fn end_of_quoted(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// End of the dollar-quoted string (`$$...$$`, `$tag$...$tag$`) starting at
/// `start`, or `None` if there is none there (e.g. a `$1` parameter)
fn end_of_dollar_quoted(sql: &str, start: usize) -> Option<usize> {
    let rest = sql.get(start..)?;
    if !rest.starts_with('$') {
        return None;
    }
    let tag_len = rest[1..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let tag_body = &rest[1..=tag_len];
    if tag_body.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !rest[1 + tag_len..].starts_with('$')
    {
        return None;
    }
    let delimiter = &rest[..tag_len + 2];
    let body_start = start + delimiter.len();
    Some(
        sql[body_start..]
            .find(delimiter)
            .map_or(sql.len(), |n| body_start + n + delimiter.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{DiagnosticKind, Span};

    fn diag(kind: DiagnosticKind, line: usize, column: usize) -> Diagnostic {
        Diagnostic::error(kind, format!("problem at {line}:{column}"))
            .with_span(Span::with_location(line, column, 1))
    }

    #[test]
    fn fnv1a_matches_reference_values() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn statement_hash_is_stable() {
        // Changing this value breaks every existing baseline file
        assert_eq!(statement_hash(" SELECT\n 1 ;"), "199e7bca63ea84f2");
    }

    #[test]
    fn normalization_ignores_whitespace_and_comments_but_not_strings() {
        assert_eq!(
            normalize_statement("  SELECT  a,\n\tb /* x; y */ FROM t -- c;\n ;"),
            "SELECT a, b FROM t"
        );
        assert_eq!(normalize_statement("SELECT 'a  b'"), "SELECT 'a  b'");
        assert_ne!(
            statement_hash("SELECT 'a  b'"),
            statement_hash("SELECT 'a b'")
        );
    }

    #[test]
    fn statements_split_on_semicolons_outside_quotes_and_comments() {
        let sql = "SELECT ';' FROM t; -- a;b\nSELECT $$ ; $$, $1 FROM u; /* ; */ SELECT \"a;\"";
        let ranges = statement_ranges(sql);
        let texts: Vec<String> = ranges
            .iter()
            .map(|r| normalize_statement(&sql[r.clone()]))
            .collect();
        assert_eq!(
            texts,
            [
                "SELECT ';' FROM t",
                "SELECT $$ ; $$, $1 FROM u",
                "SELECT \"a;\""
            ]
        );
    }

    #[test]
    fn dollar_quote_tags() {
        assert_eq!(end_of_dollar_quoted("$fn$ a; $fn$;", 0), Some(12));
        assert_eq!(end_of_dollar_quoted("$1", 0), None);
        assert_eq!(end_of_dollar_quoted("$a", 0), None);
    }

    #[test]
    fn diagnostics_in_unchanged_statements_survive_inserted_lines() {
        let before = "SELECT a FROM t;\nSELECT b FROM t;\n";
        let after = "SELECT new FROM t;\n\n-- comment\nSELECT a FROM t;\nSELECT b FROM t;\n";
        let mut baseline = Baseline::default();
        baseline.add_file(
            "q.sql",
            before,
            &[
                diag(DiagnosticKind::ColumnNotFound, 1, 8),
                diag(DiagnosticKind::ColumnNotFound, 2, 8),
            ],
        );
        let filter = BaselineFilter::new(&baseline);
        let filtered = filter.filter(
            "q.sql",
            after,
            vec![
                diag(DiagnosticKind::ColumnNotFound, 1, 8),
                diag(DiagnosticKind::ColumnNotFound, 4, 8),
                diag(DiagnosticKind::ColumnNotFound, 5, 8),
            ],
        );
        assert_eq!(filtered.kept.len(), 1);
        assert_eq!(filtered.kept[0].span.unwrap().line, 1);
        assert_eq!(filtered.suppressed.len(), 2);
    }

    #[test]
    fn occurrences_are_counted_per_statement_and_code() {
        let sql = "SELECT a, b FROM t;";
        let mut baseline = Baseline::default();
        baseline.add_file("q.sql", sql, &[diag(DiagnosticKind::ColumnNotFound, 1, 8)]);
        let filtered = BaselineFilter::new(&baseline).filter(
            "q.sql",
            sql,
            vec![
                diag(DiagnosticKind::ColumnNotFound, 1, 8),
                diag(DiagnosticKind::ColumnNotFound, 1, 11),
                diag(DiagnosticKind::TypeMismatch, 1, 11),
            ],
        );
        // The second E0002 in the statement is new, and so is the E0003
        assert_eq!(filtered.kept.len(), 2);
        assert_eq!(filtered.suppressed[0].occurrence, 0);
    }

    #[test]
    fn changed_statement_or_other_file_is_not_matched() {
        let mut baseline = Baseline::default();
        baseline.add_file(
            "q.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
        );
        let filter = BaselineFilter::new(&baseline);
        let d = || vec![diag(DiagnosticKind::ColumnNotFound, 1, 8)];
        assert_eq!(
            filter.filter("q.sql", "SELECT a FROM u;", d()).kept.len(),
            1
        );
        assert_eq!(
            filter.filter("r.sql", "SELECT a FROM t;", d()).kept.len(),
            1
        );
        // Reformatting and path spelling don't matter
        assert!(filter
            .filter("./q.sql", "SELECT a\n  FROM t;", d())
            .kept
            .is_empty());
    }

    #[test]
    fn diagnostics_without_span_use_an_empty_hash() {
        let d = Diagnostic::error(DiagnosticKind::ParseError, "no location");
        let fp = fingerprints("q.sql", "SELECT 1", &[d]);
        assert_eq!(fp[0].statement_hash, "");
    }

    #[test]
    fn stale_entries_count_only_checked_files() {
        let mut baseline = Baseline::default();
        baseline.add_file(
            "a.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
        );
        baseline.add_file(
            "b.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
        );
        let filter = BaselineFilter::new(&baseline);
        let checked: HashSet<String> = ["a.sql".to_string()].into();
        assert_eq!(filter.stale(&checked, &HashSet::new()), 1);
    }

    #[test]
    fn json_round_trip_and_version_check() {
        let mut baseline = Baseline::default();
        baseline.add_file(
            "b.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
        );
        baseline.add_file(
            "a.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::TableNotFound, 1, 15)],
        );
        let json = baseline.to_json();
        let parsed = Baseline::from_json(&json).unwrap();
        assert_eq!(parsed.entries[0].file, "a.sql");
        assert_eq!(parsed.entries.len(), 2);
        assert!(Baseline::from_json("{\"version\": 99, \"entries\": []}")
            .unwrap_err()
            .contains("unsupported baseline version"));
        assert!(Baseline::from_json("not json").is_err());
    }

    #[test]
    fn file_keys_are_relative_to_the_base_dir() {
        let base = std::env::current_dir().unwrap().join("proj");
        assert_eq!(
            file_key(&base.join("sql").join("q.sql"), &base),
            "sql/q.sql"
        );
        assert_eq!(normalize_separators(".\\sql\\q.sql"), "sql/q.sql");
    }
}
