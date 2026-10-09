//! Baselines: record the diagnostics a project already has, so that only new
//! ones are reported (`sqlsift check --write-baseline`, `baseline` in `sqlsift.toml`)
//!
//! Each entry of a baseline names:
//!
//! - the file, relative to the baseline file's directory with `/` separators
//!   (see [`file_key`]; symbolic links are resolved)
//! - the rule code
//! - a hash of the statement containing the diagnostic (see [`statement_hash`]):
//!   comments, whitespace and the case of keywords and identifiers are
//!   ignored, so that editing other statements, adding lines above it or
//!   reformatting it keeps the match. In TypeScript / JavaScript files only
//!   the SQL template is hashed (see [`sql_text`]).
//! - the message (for people reading the file, and for the fallback below)
//!
//! A file's diagnostics are matched to its entries in two passes, each entry
//! hiding at most one diagnostic:
//!
//! 1. by rule code and statement hash (the same mistake made twice in one
//!    statement needs two entries)
//! 2. the diagnostics left, by rule code and message, against the entries
//!    left: when a statement with several baselined problems is edited (one of
//!    them fixed), its other problems stay hidden
//!
//! Entries are sorted by file, code, hash and message and store no line
//! numbers, so that adding lines to a file doesn't change the baseline file.
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
//! // Lines added above it and reformatting don't matter; a new problem is
//! // still reported
//! let new = "SELECT id FROM users;\n\nselect nme from users;\nSELECT x FROM users;";
//! let filtered = BaselineFilter::new(&baseline).filter("q.sql", new, analyze(new));
//! assert_eq!(filtered.kept.len(), 1);
//! assert!(filtered.kept[0].message.contains("'x'"));
//! assert_eq!(filtered.suppressed.len(), 1);
//! ```

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dialect::SqlDialect;
use crate::error::Diagnostic;

/// File name used when no baseline path is configured
pub const DEFAULT_BASELINE_FILE: &str = "sqlsift-baseline.json";

/// Version of the baseline file format written by this version of sqlsift.
///
/// - 1: statement hashes ignore comments and whitespace only; entries also
///   store the line and the occurrence index of the diagnostic
/// - 2: statement hashes also ignore the case of keywords and identifiers and
///   whitespace next to punctuation; no line or occurrence index
///
/// Version 1 files are still read (their hashes are computed the version 1 way).
pub const BASELINE_VERSION: u32 = 2;

/// The oldest baseline file format that is read
const OLDEST_VERSION: u32 = 1;

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
    /// Message of the diagnostic
    #[serde(default)]
    pub message: String,
}

/// What [`Baseline::keep_unchecked`] did with the entries of the files that
/// were not checked
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeptEntries {
    /// Entries kept
    pub kept: usize,
    /// Files whose entries were kept
    pub kept_files: usize,
    /// Entries dropped because their file no longer exists or is ignored
    pub removed: usize,
    /// Files whose entries were dropped
    pub removed_files: usize,
    /// Kept entries from an older file format (their statement hashes are of
    /// the old kind, so they only match by message)
    pub old_format: usize,
}

impl Baseline {
    /// Parse a baseline file
    pub fn from_json(json: &str) -> Result<Self, String> {
        let baseline: Self =
            serde_json::from_str(json).map_err(|e| format!("invalid baseline file: {e}"))?;
        if !(OLDEST_VERSION..=BASELINE_VERSION).contains(&baseline.version) {
            return Err(format!(
                "unsupported baseline version {} (expected {BASELINE_VERSION}); re-create it with --write-baseline",
                baseline.version
            ));
        }
        Ok(baseline)
    }

    /// Serialize as pretty-printed JSON (with a trailing newline), entries
    /// sorted by file, code, statement hash and message, so that the file
    /// diffs well and doesn't change when lines move
    pub fn to_json(&self) -> String {
        let mut sorted = self.clone();
        sorted.entries.sort_by(|a, b| {
            (&a.file, &a.code, &a.statement_hash, &a.message).cmp(&(
                &b.file,
                &b.code,
                &b.statement_hash,
                &b.message,
            ))
        });
        let mut json = serde_json::to_string_pretty(&sorted).unwrap_or_default();
        json.push('\n');
        json
    }

    /// Add the diagnostics of a file. `file` is its [`file_key`]; `sql` is the
    /// SQL the diagnostics were reported on (see [`sql_text`]).
    pub fn add_file(&mut self, file: &str, sql: &str, diagnostics: &[Diagnostic]) {
        let file = normalize_separators(file);
        let hashes = statement_hashes(self.version, sql, diagnostics);
        for (diagnostic, statement_hash) in diagnostics.iter().zip(hashes) {
            self.entries.push(BaselineEntry {
                file: file.clone(),
                code: diagnostic.code().to_string(),
                statement_hash,
                message: diagnostic.message.clone(),
            });
        }
    }

    /// Add the entries of `previous` (the baseline file being replaced) for the
    /// files not in `checked` ([`file_key`]s), except those for which
    /// `is_gone` (given an entry's file) says the file no longer exists or is
    /// ignored. Used to update a baseline from a run over some of its files.
    pub fn keep_unchecked(
        &mut self,
        previous: &Baseline,
        checked: &HashSet<String>,
        mut is_gone: impl FnMut(&str) -> bool,
    ) -> KeptEntries {
        let mut summary = KeptEntries::default();
        let mut gone: HashMap<String, bool> = HashMap::new();
        let mut kept_files = HashSet::new();
        for entry in &previous.entries {
            let file = normalize_separators(&entry.file);
            if checked.contains(&file) {
                continue;
            }
            let file_gone = *gone.entry(file.clone()).or_insert_with(|| is_gone(&file));
            if file_gone {
                summary.removed += 1;
                continue;
            }
            summary.kept += 1;
            if previous.version != self.version {
                summary.old_format += 1;
            }
            kept_files.insert(file.clone());
            self.entries.push(BaselineEntry {
                file,
                ..entry.clone()
            });
        }
        summary.kept_files = kept_files.len();
        summary.removed_files = gone.values().filter(|g| **g).count();
        summary
    }
}

/// Diagnostics of a file after removing the baselined ones
#[derive(Debug, Default)]
pub struct Filtered {
    /// Diagnostics not in the baseline
    pub kept: Vec<Diagnostic>,
    /// Diagnostics hidden by a baseline entry
    pub suppressed: Vec<Diagnostic>,
    /// The file's baseline entries that matched no diagnostic (problems fixed
    /// since the baseline was written)
    pub stale: Vec<BaselineEntry>,
}

/// Removes baselined diagnostics
#[derive(Debug, Clone, Default)]
pub struct BaselineFilter {
    /// Format version of the baseline (how its statement hashes are computed)
    version: u32,
    /// Entries by file
    files: HashMap<String, Vec<BaselineEntry>>,
}

impl BaselineFilter {
    pub fn new(baseline: &Baseline) -> Self {
        let mut files: HashMap<String, Vec<BaselineEntry>> = HashMap::new();
        for entry in &baseline.entries {
            files
                .entry(normalize_separators(&entry.file))
                .or_default()
                .push(entry.clone());
        }
        Self {
            version: baseline.version,
            files,
        }
    }

    /// Number of entries
    pub fn len(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Split the diagnostics of a file into new and baselined ones (see the
    /// module docs). `file` is its [`file_key`]; `sql` is the SQL the
    /// diagnostics were reported on (see [`sql_text`]).
    pub fn filter(&self, file: &str, sql: &str, diagnostics: Vec<Diagnostic>) -> Filtered {
        let entries = self
            .files
            .get(&normalize_separators(file))
            .map_or(&[][..], Vec::as_slice);
        if entries.is_empty() {
            return Filtered {
                kept: diagnostics,
                ..Filtered::default()
            };
        }
        let hashes = statement_hashes(self.version, sql, &diagnostics);
        let mut used = vec![false; entries.len()];
        let mut matched = vec![false; diagnostics.len()];

        // By code and statement hash (each key holds the entries left to use,
        // first one last)
        let mut by_hash: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for (j, entry) in entries.iter().enumerate().rev() {
            by_hash
                .entry((&entry.code, &entry.statement_hash))
                .or_default()
                .push(j);
        }
        for (i, diagnostic) in diagnostics.iter().enumerate() {
            if let Some(j) = by_hash
                .get_mut(&(diagnostic.code(), hashes[i].as_str()))
                .and_then(Vec::pop)
            {
                used[j] = true;
                matched[i] = true;
            }
        }

        // The rest by code and message
        let mut by_message: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for (j, entry) in entries.iter().enumerate().rev() {
            if !used[j] {
                by_message
                    .entry((&entry.code, &entry.message))
                    .or_default()
                    .push(j);
            }
        }
        for (i, diagnostic) in diagnostics.iter().enumerate() {
            if matched[i] {
                continue;
            }
            if let Some(j) = by_message
                .get_mut(&(diagnostic.code(), diagnostic.message.as_str()))
                .and_then(Vec::pop)
            {
                used[j] = true;
                matched[i] = true;
            }
        }

        let mut filtered = Filtered::default();
        for (diagnostic, matched) in diagnostics.into_iter().zip(matched) {
            if matched {
                filtered.suppressed.push(diagnostic);
            } else {
                filtered.kept.push(diagnostic);
            }
        }
        filtered.stale = entries
            .iter()
            .zip(used)
            .filter(|(_, used)| !used)
            .map(|(entry, _)| entry.clone())
            .collect();
        filtered
    }

    /// The entries of the files not in `checked` ([`file_key`]s) that are
    /// stale because `is_gone` (given an entry's file) says the file no longer
    /// exists or is ignored. Entries of other files that weren't checked are
    /// not stale: the run just didn't cover them.
    pub fn stale_unchecked(
        &self,
        checked: &HashSet<String>,
        mut is_gone: impl FnMut(&str) -> bool,
    ) -> Vec<&BaselineEntry> {
        let mut files: Vec<_> = self
            .files
            .iter()
            .filter(|(file, _)| !checked.contains(*file) && is_gone(file))
            .collect();
        files.sort_by(|a, b| a.0.cmp(b.0));
        files.into_iter().flat_map(|(_, entries)| entries).collect()
    }
}

/// The SQL that the diagnostics of a file are reported on, to give to
/// [`Baseline`] and [`BaselineFilter`]: the file's text, or for TypeScript /
/// JavaScript files (by `name`, see
/// [`is_embedded_sql_file`](crate::embedded::is_embedded_sql_file)) the SQL of
/// its templates tagged with one of `tags`, so that the code around a
/// template is not part of its statement.
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::baseline::sql_text;
/// use sqlsift_core::SqlDialect;
///
/// let ts = "const rows = await sql`SELECT 1`;";
/// let sql = sql_text(Path::new("a.ts"), ts, &["sql"], SqlDialect::PostgreSQL);
/// assert_eq!(sql.trim(), ";SELECT 1;");
/// ```
pub fn sql_text<S: AsRef<str>>(
    name: &Path,
    source: &str,
    tags: &[S],
    dialect: SqlDialect,
) -> String {
    if crate::embedded::is_embedded_sql_file(name) {
        // As `Analyzer::analyze_embedded`: the BOM is not part of the script
        let (script, bom) = crate::analyzer::strip_bom(source);
        let host = if crate::embedded::is_component_file(name) {
            crate::embedded::Host::Component
        } else {
            crate::embedded::Host::Script
        };
        let text = crate::embedded::extract(script, tags, dialect, host).text;
        format!("{}{text}", &source[..bom])
    } else {
        source.to_string()
    }
}

/// The name of `path` in a baseline: relative to `base_dir` (the directory
/// containing the baseline file), with `/` separators, so that it is the same
/// whatever directory sqlsift runs in (and in the editor). Relative paths are
/// taken relative to the current directory, and symbolic links are resolved
/// (in the part of the path that exists), so a file reached through a
/// symlinked directory has the same name. A path that shares no root with
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
    let absolute = resolve(path);
    let base = resolve(base_dir);
    let path_parts: Vec<_> = absolute.components().collect();
    let base_parts: Vec<_> = base.components().collect();
    let common = path_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return normalize_separators(&absolute.to_string_lossy());
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

/// `path` made absolute, lexically normalized, and with symbolic links
/// resolved in the part of it that exists
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // Normalized first so that a `..` never ends the walk below early
    let absolute = crate::ignore::normalize(&absolute);
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            let joined = rest
                .iter()
                .rev()
                .fold(strip_verbatim(canonical), |path: PathBuf, part| {
                    path.join(part)
                });
            return crate::ignore::normalize(&joined);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return crate::ignore::normalize(&absolute),
        }
    }
}

/// `path` without the `\\?\` prefix that `canonicalize` adds on Windows, so
/// it compares equal to paths that were not canonicalized
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
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

/// Statement hashes (of baseline format `version`) of `diagnostics`, in the
/// same order
fn statement_hashes(version: u32, sql: &str, diagnostics: &[Diagnostic]) -> Vec<String> {
    let ranges = statement_ranges(sql);
    let lines = line_starts(sql);
    let mut hashes: HashMap<Range<usize>, String> = HashMap::new();
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic
                .span
                .map(|span| {
                    let offset = if span.line > 0 {
                        byte_offset(sql, &lines, span.line, span.column)
                    } else {
                        span.offset.min(sql.len())
                    };
                    let range = enclosing(&ranges, offset);
                    hashes
                        .entry(range.clone())
                        .or_insert_with(|| hash_version(version, &sql[range]))
                        .clone()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// Hash of a statement as written to baseline files (FNV-1a, 64 bit, as 16 hex
/// digits; stable across platforms and versions) of its tokens, normalized:
///
/// - comments and the trailing `;` removed
/// - keywords and unquoted identifiers lowercased; a quoted identifier that is
///   lowercase and needs no quotes (`"users"`) unquoted
/// - whitespace kept only between words (one space), not next to operators,
///   parentheses or commas
/// - string literals exactly as written
///
/// ```
/// use sqlsift_core::baseline::statement_hash;
///
/// assert_eq!(
///     statement_hash("SELECT a\n  FROM t WHERE id = ( 1 ); -- note"),
///     statement_hash("-- a comment\nselect A from \"t\" where id=(1)"),
/// );
/// assert_ne!(statement_hash("SELECT a FROM t"), statement_hash("SELECT b FROM t"));
/// assert_ne!(statement_hash("SELECT 'A'"), statement_hash("SELECT 'a'"));
/// ```
pub fn statement_hash(statement: &str) -> String {
    hash_version(BASELINE_VERSION, statement)
}

/// [`statement_hash`] as computed by baseline format `version`
fn hash_version(version: u32, statement: &str) -> String {
    let normalized = if version == 1 {
        normalize_v1(statement)
    } else {
        normalize_statement(statement)
    };
    format!("{:016x}", fnv1a_64(normalized.as_bytes()))
}

/// FNV-1a, 64 bit
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

/// The normalized text of a statement that [`statement_hash`] hashes
fn normalize_statement(statement: &str) -> String {
    let mut out = String::with_capacity(statement.len());
    let mut pending_space = false;
    let mut after_word = false;
    for token in tokens(statement) {
        let text = &statement[token.range];
        match token.kind {
            TokenKind::Whitespace | TokenKind::Comment => pending_space = true,
            TokenKind::Semicolon => {}
            TokenKind::Punctuation => {
                out.push_str(text);
                after_word = false;
                pending_space = false;
            }
            TokenKind::Word | TokenKind::QuotedIdentifier | TokenKind::String => {
                if pending_space && after_word {
                    out.push(' ');
                }
                match token.kind {
                    TokenKind::Word => out.push_str(&text.to_lowercase()),
                    TokenKind::QuotedIdentifier => out.push_str(unquote_identifier(text)),
                    _ => out.push_str(text),
                }
                after_word = true;
                pending_space = false;
            }
        }
    }
    out
}

/// A quoted identifier without its quotes when that doesn't change its
/// meaning (in PostgreSQL): it is lowercase and needs no quoting
fn unquote_identifier(quoted: &str) -> &str {
    let inner = if quoted.len() >= 2 && quoted.ends_with(&quoted[..1]) {
        &quoted[1..quoted.len() - 1]
    } else {
        ""
    };
    let simple = inner
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && inner
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if simple {
        inner
    } else {
        quoted
    }
}

/// The normalization of baseline format 1: comments removed, runs of
/// whitespace (outside quotes) collapsed to one space, without the trailing `;`
fn normalize_v1(statement: &str) -> String {
    let mut out = String::with_capacity(statement.len());
    let mut pending_space = false;
    for token in tokens(statement) {
        match token.kind {
            TokenKind::Whitespace | TokenKind::Comment => pending_space = true,
            TokenKind::Semicolon => {}
            _ => {
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
    /// Whitespace, including a byte order mark
    Whitespace,
    Comment,
    Semicolon,
    /// A keyword, unquoted identifier, number or parameter (`$1`)
    Word,
    /// `"name"` or `` `name` ``
    QuotedIdentifier,
    /// `'text'` or a dollar-quoted string
    String,
    /// Any other character (operators, parentheses, commas, ...)
    Punctuation,
}

#[derive(Debug)]
struct Token {
    kind: TokenKind,
    range: Range<usize>,
}

fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Split SQL into tokens. Dialect neutral and deliberately simple: quotes
/// (`'`, `"`, `` ` ``, with doubled quotes as escapes), dollar-quoted strings
/// and `--` / `/* */` comments are recognized, so that a `;` or `--` inside
/// them doesn't split the statement. Every token boundary is a char boundary,
/// whatever the text.
fn tokens(sql: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = sql[i..].chars().next() {
        let start = i;
        let rest = &sql[i..];
        let run = |pred: fn(char) -> bool| rest.find(|c| !pred(c)).unwrap_or(rest.len());
        let kind = if is_space(c) {
            i += run(is_space);
            TokenKind::Whitespace
        } else if c == ';' {
            i += 1;
            TokenKind::Semicolon
        } else if rest.starts_with("--") {
            i += rest.find('\n').unwrap_or(rest.len());
            TokenKind::Comment
        } else if let Some(body) = rest.strip_prefix("/*") {
            i += body.find("*/").map_or(rest.len(), |n| n + 4);
            TokenKind::Comment
        } else if c == '\'' {
            i = end_of_quoted(sql.as_bytes(), i);
            TokenKind::String
        } else if matches!(c, '"' | '`') {
            i = end_of_quoted(sql.as_bytes(), i);
            TokenKind::QuotedIdentifier
        } else if let Some(end) = end_of_dollar_quoted(sql, i) {
            i = end;
            TokenKind::String
        } else if is_word_char(c) {
            i += run(is_word_char);
            TokenKind::Word
        } else {
            i += c.len_utf8();
            TokenKind::Punctuation
        };
        tokens.push(Token {
            kind,
            range: start..i,
        });
    }
    tokens
}

/// End of the quoted text starting at `start` (a doubled quote is an escape).
/// The quote is ASCII, so the end is a char boundary.
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

    fn diag_msg(kind: DiagnosticKind, line: usize, column: usize, message: &str) -> Diagnostic {
        Diagnostic::error(kind, message).with_span(Span::with_location(line, column, 1))
    }

    #[test]
    fn fnv1a_matches_reference_values() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn statement_hash_is_stable() {
        // Changing these values breaks every existing baseline file
        assert_eq!(hash_version(1, " SELECT\n 1 ;"), "199e7bca63ea84f2");
        assert_eq!(statement_hash(" SELECT\n 1 ;"), "02fb785a1a5a96f2");
    }

    #[test]
    fn normalization_ignores_whitespace_and_comments_but_not_strings() {
        assert_eq!(
            normalize_statement("  SELECT  a,\n\tb /* x; y */ FROM t -- c;\n ;"),
            "select a,b from t"
        );
        assert_eq!(normalize_statement("SELECT 'a  B'"), "select 'a  B'");
        assert_ne!(
            statement_hash("SELECT 'a  b'"),
            statement_hash("SELECT 'a b'")
        );
        assert_ne!(statement_hash("SELECT 'A'"), statement_hash("SELECT 'a'"));
    }

    #[test]
    fn normalization_is_robust_to_formatters() {
        let same = [
            "SELECT * FROM customers WHERE customer_id = 993",
            "select * from customers where customer_id=993",
            "SELECT *\n  FROM \"customers\"\n WHERE customer_id = 993;",
            "SELECT * FROM `customers` WHERE Customer_Id =993",
        ];
        for sql in same {
            assert_eq!(
                normalize_statement(sql),
                "select*from customers where customer_id=993",
                "{sql}"
            );
        }
        assert_eq!(
            normalize_statement("SELECT COUNT( * ) FROM t WHERE x IN ( 1 , 2 )"),
            normalize_statement("select count(*) from t where x in (1,2)")
        );
        // Quoting that matters is kept
        assert_eq!(normalize_statement("SELECT \"Name\""), "select \"Name\"");
        assert_eq!(normalize_statement("SELECT \"a b\""), "select \"a b\"");
        // Words stay separated
        assert_ne!(
            normalize_statement("SELECT a b FROM t"),
            normalize_statement("SELECT ab FROM t")
        );
    }

    #[test]
    fn v1_normalization_is_unchanged() {
        assert_eq!(
            normalize_v1("  SELECT  a,\n\tb /* x; y */ FROM t -- c;\n ;"),
            "SELECT a, b FROM t"
        );
        assert_eq!(
            normalize_v1("SELECT a=b, $1, $$x$$"),
            "SELECT a=b, $1, $$x$$"
        );
    }

    #[test]
    fn non_ascii_text_does_not_panic() {
        let sources = [
            "\u{feff}SELECT 1;\nSELECT frist_name FROM actor;\n",
            "SELECT frist_name FROM café;\n",
            "SELECT 名前 FROM 顧客 WHERE 番号 = 1;\n",
            "SELECT x FROM t WHERE note = '🎉 done' AND y=1;\n",
            "SELECT é-ü, a—b FROM t;\n",
        ];
        for sql in sources {
            let d = vec![diag(DiagnosticKind::ColumnNotFound, sql.lines().count(), 8)];
            let mut baseline = Baseline::default();
            baseline.add_file("q.sql", sql, &d);
            assert_eq!(baseline.entries[0].statement_hash.len(), 16);
            let filtered = BaselineFilter::new(&baseline).filter("q.sql", sql, d);
            assert!(filtered.kept.is_empty(), "{sql}");
            let _ = normalize_v1(sql);
        }
        // A byte order mark is whitespace
        assert_eq!(
            statement_hash("\u{feff}SELECT 1"),
            statement_hash("SELECT 1")
        );
        assert_eq!(
            normalize_statement("SELECT Café FROM 顧客"),
            "select café from 顧客"
        );
        assert_eq!(normalize_statement("SELECT '🎉  Ünï'"), "select '🎉  Ünï'");
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
                "select ';' from t",
                "select $$ ; $$,$1 from u",
                "select \"a;\""
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
                diag_msg(DiagnosticKind::ColumnNotFound, 1, 8, "a"),
                diag_msg(DiagnosticKind::ColumnNotFound, 2, 8, "b"),
            ],
        );
        let filter = BaselineFilter::new(&baseline);
        let filtered = filter.filter(
            "q.sql",
            after,
            vec![
                diag_msg(DiagnosticKind::ColumnNotFound, 1, 8, "new"),
                diag_msg(DiagnosticKind::ColumnNotFound, 4, 8, "a"),
                diag_msg(DiagnosticKind::ColumnNotFound, 5, 8, "b"),
            ],
        );
        assert_eq!(filtered.kept.len(), 1);
        assert_eq!(filtered.kept[0].span.unwrap().line, 1);
        assert_eq!(filtered.suppressed.len(), 2);
        assert!(filtered.stale.is_empty());
    }

    #[test]
    fn each_entry_hides_one_diagnostic() {
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
        assert_eq!(filtered.suppressed.len(), 1);
        assert_eq!(filtered.suppressed[0].span.unwrap().column, 8);
    }

    #[test]
    fn fixing_one_of_several_problems_in_a_statement_keeps_the_others_hidden() {
        let before = "SELECT frist_name, frist_name, lst_name FROM actor WHERE actor_id = 'abc';";
        let after = "SELECT frist_name, frist_name, last_name FROM actor WHERE actor_id = 'abc';";
        let frist = |col| diag_msg(DiagnosticKind::ColumnNotFound, 1, col, "frist_name");
        let mismatch = || diag_msg(DiagnosticKind::TypeMismatch, 1, 60, "type mismatch");
        let mut baseline = Baseline::default();
        baseline.add_file(
            "q.sql",
            before,
            &[
                frist(8),
                frist(20),
                diag_msg(DiagnosticKind::ColumnNotFound, 1, 32, "lst_name"),
                mismatch(),
            ],
        );
        let filter = BaselineFilter::new(&baseline);
        let filtered = filter.filter("q.sql", after, vec![frist(8), frist(20), mismatch()]);
        assert!(filtered.kept.is_empty(), "{:?}", filtered.kept);
        assert_eq!(filtered.suppressed.len(), 3);
        assert_eq!(filtered.stale.len(), 1);
        assert_eq!(filtered.stale[0].message, "lst_name");

        // The message fallback counts too: a third `frist_name` is new
        let filtered = filter.filter("q.sql", after, vec![frist(8), frist(20), frist(40)]);
        assert_eq!(filtered.kept.len(), 1);
    }

    #[test]
    fn changed_statement_or_other_file_is_not_matched() {
        let mut baseline = Baseline::default();
        baseline.add_file(
            "q.sql",
            "SELECT a FROM t;",
            &[diag_msg(DiagnosticKind::ColumnNotFound, 1, 8, "a")],
        );
        let filter = BaselineFilter::new(&baseline);
        let d = |m| vec![diag_msg(DiagnosticKind::ColumnNotFound, 1, 8, m)];
        assert_eq!(
            filter
                .filter("q.sql", "SELECT a FROM u;", d("x"))
                .kept
                .len(),
            1
        );
        assert_eq!(
            filter
                .filter("r.sql", "SELECT a FROM t;", d("a"))
                .kept
                .len(),
            1
        );
        // Reformatting and path spelling don't matter
        assert!(filter
            .filter("./q.sql", "select a\n  FROM T;", d("x"))
            .kept
            .is_empty());
    }

    #[test]
    fn diagnostics_without_span_use_an_empty_hash() {
        let d = Diagnostic::error(DiagnosticKind::ParseError, "no location");
        let mut baseline = Baseline::default();
        baseline.add_file("q.sql", "SELECT 1", &[d]);
        assert_eq!(baseline.entries[0].statement_hash, "");
    }

    #[test]
    fn version_1_baselines_still_match() {
        let json = format!(
            r#"{{"version": 1, "entries": [{{"file": "q.sql", "code": "E0002",
                "statement_hash": "{}", "occurrence": 0, "line": 1, "message": "old"}}]}}"#,
            hash_version(1, "SELECT a FROM t;")
        );
        let baseline = Baseline::from_json(&json).unwrap();
        assert_eq!(baseline.version, 1);
        let filter = BaselineFilter::new(&baseline);
        let filtered = filter.filter(
            "q.sql",
            "SELECT id FROM t;\nSELECT a\nFROM t;",
            vec![diag_msg(DiagnosticKind::ColumnNotFound, 2, 8, "new")],
        );
        assert!(filtered.kept.is_empty());
    }

    #[test]
    fn stale_entries_of_unchecked_files_only_when_gone() {
        let mut baseline = Baseline::default();
        for file in ["a.sql", "b.sql", "gone.sql"] {
            baseline.add_file(
                file,
                "SELECT a FROM t;",
                &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
            );
        }
        let filter = BaselineFilter::new(&baseline);
        let checked: HashSet<String> = ["a.sql".to_string()].into();
        let stale = filter.stale_unchecked(&checked, |f| f == "gone.sql" || f == "a.sql");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].file, "gone.sql");
    }

    #[test]
    fn keep_unchecked_merges_entries_of_other_files() {
        let mut previous = Baseline::default();
        for file in ["a.sql", "b.sql", "gone.sql"] {
            previous.add_file(
                file,
                "SELECT a FROM t;",
                &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
            );
        }
        let mut baseline = Baseline::default();
        // a.sql was checked and is clean now
        let checked: HashSet<String> = ["a.sql".to_string()].into();
        let summary = baseline.keep_unchecked(&previous, &checked, |f| f == "gone.sql");
        assert_eq!(
            summary,
            KeptEntries {
                kept: 1,
                kept_files: 1,
                removed: 1,
                removed_files: 1,
                old_format: 0
            }
        );
        let files: Vec<&str> = baseline.entries.iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["b.sql"]);
    }

    #[test]
    fn json_round_trip_sorting_and_version_check() {
        let mut baseline = Baseline::default();
        baseline.add_file(
            "b.sql",
            "SELECT a FROM t;",
            &[diag(DiagnosticKind::ColumnNotFound, 1, 8)],
        );
        baseline.add_file(
            "a.sql",
            "SELECT x FROM y;\nSELECT a FROM t;",
            &[
                diag(DiagnosticKind::TableNotFound, 2, 15),
                diag(DiagnosticKind::ColumnNotFound, 1, 8),
            ],
        );
        let json = baseline.to_json();
        assert!(!json.contains("\"line\""), "{json}");
        let parsed = Baseline::from_json(&json).unwrap();
        let order: Vec<(&str, &str)> = parsed
            .entries
            .iter()
            .map(|e| (e.file.as_str(), e.code.as_str()))
            .collect();
        assert_eq!(
            order,
            [("a.sql", "E0001"), ("a.sql", "E0002"), ("b.sql", "E0002")]
        );
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
        assert_eq!(
            file_key(&base.join("ci").join("..").join("q.sql"), &base),
            "q.sql"
        );
        assert_eq!(normalize_separators(".\\sql\\q.sql"), "sql/q.sql");
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\proj\q.sql")),
            PathBuf::from(r"C:\proj\q.sql")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\host\share\q.sql")),
            PathBuf::from(r"\\host\share\q.sql")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/proj/q.sql")),
            PathBuf::from("/proj/q.sql")
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_keys_resolve_symbolic_links() {
        let dir = std::env::temp_dir().join(format!("sqlsift-bl-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/queries")).unwrap();
        std::fs::write(dir.join("repo/queries/q.sql"), "SELECT 1").unwrap();
        std::os::unix::fs::symlink(dir.join("repo/queries"), dir.join("repo/qlink")).unwrap();
        let base = dir.join("repo");
        assert_eq!(file_key(&base.join("qlink/q.sql"), &base), "queries/q.sql");
        assert_eq!(
            file_key(&base.join("queries/q.sql"), &base),
            "queries/q.sql"
        );
        // A file that doesn't exist (yet) below a symlinked directory
        assert_eq!(
            file_key(&base.join("qlink/new.sql"), &base),
            "queries/new.sql"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn embedded_sql_is_hashed_without_the_code_around_it() {
        let hash_of = |source: &str| {
            let sql = sql_text(Path::new("a.ts"), source, &["sql"], SqlDialect::PostgreSQL);
            let mut baseline = Baseline::default();
            let line = 2;
            let column = source.lines().nth(1).unwrap().find("frist").unwrap() + 1;
            baseline.add_file(
                "a.ts",
                &sql,
                &[diag(DiagnosticKind::ColumnNotFound, line, column)],
            );
            baseline.entries[0].statement_hash.clone()
        };
        let before = "import { sql } from './db';\nconst rows = await db.query(sql`SELECT frist_name FROM actor WHERE actor_id = ${id}`);\n";
        let after = "import { sql } from './db';\nconst result = await db.query(sql`SELECT frist_name FROM actor WHERE actor_id = ${actorId}`);\n";
        assert_eq!(hash_of(before), hash_of(after));
        assert_eq!(
            hash_of(before),
            statement_hash("SELECT frist_name FROM actor WHERE actor_id = $1")
        );
    }
}
