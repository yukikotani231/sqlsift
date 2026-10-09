//! Ignore patterns for query files (`ignore = [...]` in `sqlsift.toml`, `--ignore`)
//!
//! Patterns are glob patterns relative to a base directory (the directory of
//! `sqlsift.toml`, or the current directory for command line patterns):
//!
//! - `*` and `?` match within one path component, `**` matches any number of
//!   directories (`sql/archive/**`, `**/*.generated.sql`)
//! - a pattern that matches a directory ignores every file below it
//!   (`sql/archive` is the same as `sql/archive/**`)
//!
//! Paths are compared after making them absolute and lexically normalizing
//! them (removing `.` and resolving `..`), so `./sql/x.sql` and `sql/x.sql`
//! are the same file. Symbolic links are not resolved.

use std::path::{Component, Path, PathBuf};

use glob::{MatchOptions, Pattern};

/// A set of compiled ignore patterns
#[derive(Debug, Clone, Default)]
pub struct IgnorePatterns {
    patterns: Vec<Pattern>,
}

const MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

impl IgnorePatterns {
    /// Compile `patterns`, each relative to `base` (unless absolute).
    ///
    /// A relative `base` is taken relative to the current directory. Returns an
    /// error message naming the first invalid pattern.
    ///
    /// ```
    /// use std::path::Path;
    /// use sqlsift_core::ignore::IgnorePatterns;
    ///
    /// let base = Path::new("/project");
    /// let ignore = IgnorePatterns::new(base, ["sql/archive/**", "**/*.generated.sql"]).unwrap();
    /// assert!(ignore.is_ignored(Path::new("/project/sql/archive/old.sql")));
    /// assert!(ignore.is_ignored(Path::new("/project/a/b/users.generated.sql")));
    /// assert!(!ignore.is_ignored(Path::new("/project/sql/users.sql")));
    /// ```
    pub fn new<I, S>(base: &Path, patterns: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let base = normalize(base);
        let mut this = Self::default();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            this.patterns.push(compile(&base, pattern)?);
        }
        Ok(this)
    }

    /// Whether there are no patterns
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `path` (relative to the current directory unless absolute), or a
    /// directory containing it, matches one of the patterns
    pub fn is_ignored(&self, path: &Path) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let path = normalize(path);
        path.ancestors()
            .filter(|p| p.parent().is_some()) // never match the root itself
            .any(|p| {
                self.patterns
                    .iter()
                    .any(|pattern| pattern.matches_path_with(p, MATCH_OPTIONS))
            })
    }
}

/// Compile `pattern` relative to the (absolute, normalized) `base` directory
fn compile(base: &Path, pattern: &str) -> Result<Pattern, String> {
    let invalid = |e: glob::PatternError| format!("invalid ignore pattern '{pattern}': {e}");
    // Validate the pattern on its own first, so errors refer to the user's text
    Pattern::new(pattern).map_err(invalid)?;
    let full = if Path::new(pattern).is_absolute() {
        normalize(Path::new(pattern))
    } else {
        // The base directory is literal text, not a pattern
        let base = Pattern::escape(&base.to_string_lossy());
        normalize(&Path::new(&base).join(pattern))
    };
    Pattern::new(&full.to_string_lossy()).map_err(invalid)
}

/// Make `path` absolute (against the current directory) and remove `.` and `..`
/// components without touching the file system
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        // An absolute path on every platform
        std::env::current_dir().unwrap().join("proj")
    }

    fn ignored(patterns: &[&str], path: &str) -> bool {
        let base = root();
        IgnorePatterns::new(&base, patterns)
            .unwrap()
            .is_ignored(&base.join(path))
    }

    #[test]
    fn double_star_matches_nested_directories() {
        assert!(ignored(&["sql/archive/**"], "sql/archive/a.sql"));
        assert!(ignored(&["sql/archive/**"], "sql/archive/2020/a.sql"));
        assert!(!ignored(&["sql/archive/**"], "sql/current/a.sql"));
        assert!(ignored(&["**/*.generated.sql"], "x.generated.sql"));
        assert!(ignored(&["**/*.generated.sql"], "a/b/x.generated.sql"));
        assert!(!ignored(&["**/*.generated.sql"], "a/b/x.sql"));
    }

    #[test]
    fn single_star_does_not_cross_directories() {
        assert!(ignored(&["sql/*.sql"], "sql/a.sql"));
        assert!(!ignored(&["sql/*.sql"], "sql/sub/a.sql"));
    }

    #[test]
    fn directory_pattern_ignores_its_contents() {
        assert!(ignored(&["sql/archive"], "sql/archive/a.sql"));
        assert!(ignored(&["sql/archive/"], "sql/archive/x/a.sql"));
        assert!(!ignored(&["sql/archive"], "sql/archived.sql"));
    }

    #[test]
    fn patterns_are_relative_to_base() {
        let base = root();
        let ignore = IgnorePatterns::new(&base, ["a.sql"]).unwrap();
        assert!(ignore.is_ignored(&base.join("a.sql")));
        assert!(!ignore.is_ignored(&base.join("sub/a.sql")));
        assert!(!ignore.is_ignored(&base.parent().unwrap().join("a.sql")));
    }

    #[test]
    fn paths_are_normalized() {
        let base = root();
        let ignore = IgnorePatterns::new(&base, ["./sql/../gen/**"]).unwrap();
        assert!(ignore.is_ignored(&base.join("gen/./x.sql")));
        assert!(ignore.is_ignored(&base.join("other/../gen/x.sql")));
    }

    #[test]
    fn relative_paths_use_current_directory() {
        let ignore = IgnorePatterns::new(Path::new(""), ["gen/**"]).unwrap();
        assert!(ignore.is_ignored(Path::new("gen/x.sql")));
        assert!(ignore.is_ignored(Path::new("./gen/x.sql")));
        assert!(!ignore.is_ignored(Path::new("src/x.sql")));
    }

    #[test]
    fn base_directory_is_taken_literally() {
        let base = root().join("[weird] dir*");
        let ignore = IgnorePatterns::new(&base, ["*.sql"]).unwrap();
        assert!(ignore.is_ignored(&base.join("a.sql")));
        assert!(!ignore.is_ignored(&root().join("w dir/a.sql")));
    }

    #[test]
    fn invalid_pattern_is_an_error() {
        let err = IgnorePatterns::new(&root(), ["sql/[.sql"]).unwrap_err();
        assert!(err.contains("sql/[.sql"), "{err}");
    }

    #[test]
    fn empty_set_ignores_nothing() {
        let ignore = IgnorePatterns::default();
        assert!(ignore.is_empty());
        assert!(!ignore.is_ignored(Path::new("a.sql")));
    }
}
