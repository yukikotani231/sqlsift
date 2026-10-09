//! Support for migration tool conventions: rollback (down/undo) files and
//! dbmate's `-- migrate:up` / `-- migrate:down` sections.
//!
//! Only the "up" direction describes the schema, so rollback SQL must not be
//! applied when building the catalog.

use std::borrow::Cow;
use std::path::Path;

/// Whether `path` names a rollback migration that should be skipped when
/// loading a directory of migrations:
///
/// - golang-migrate / sqlx reversible migrations: `*.down.sql`
/// - Flyway undo migrations: `U<version>__<description>.sql`
///
/// ```
/// use std::path::Path;
/// use sqlsift_core::schema::is_rollback_migration;
///
/// assert!(is_rollback_migration(Path::new("000001_init.down.sql")));
/// assert!(is_rollback_migration(Path::new("U1__init.sql")));
/// assert!(!is_rollback_migration(Path::new("V1__init.sql")));
/// ```
pub fn is_rollback_migration(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".down.sql") {
        return true;
    }
    is_flyway_undo(name)
}

/// `U` followed by a version (digits, `.` or `_`, starting with a digit) and `__`
fn is_flyway_undo(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('U') else {
        return false;
    };
    let Some((version, _)) = rest.split_once("__") else {
        return false;
    };
    version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '_')
}

/// Blank out dbmate "down" sections: every line after a `-- migrate:down`
/// marker, up to the next `-- migrate:up` marker (or the end of the file).
///
/// Removed text is replaced with spaces (newlines are kept), so byte offsets
/// and line numbers of the remaining statements are unchanged. Input without
/// a down marker is returned as is.
pub fn strip_down_migrations(sql: &str) -> Cow<'_, str> {
    if !sql.contains("migrate:down") {
        return Cow::Borrowed(sql);
    }

    let mut out = String::with_capacity(sql.len());
    let mut in_down = false;
    for line in sql.split_inclusive('\n') {
        match migrate_marker(line) {
            Some(Marker::Down) => in_down = true,
            Some(Marker::Up) => in_down = false,
            None => {}
        }
        if in_down {
            out.extend(line.bytes().map(|b| if b == b'\n' { '\n' } else { ' ' }));
        } else {
            out.push_str(line);
        }
    }
    Cow::Owned(out)
}

enum Marker {
    Up,
    Down,
}

/// Recognize a dbmate marker line: `-- migrate:up` or `-- migrate:down`,
/// optionally followed by options such as `transaction:false`.
fn migrate_marker(line: &str) -> Option<Marker> {
    let rest = line.trim_start().strip_prefix("--")?.trim_start();
    let rest = rest.strip_prefix("migrate:")?;
    let (marker, after) = match rest.strip_prefix("down") {
        Some(after) => (Marker::Down, after),
        None => (Marker::Up, rest.strip_prefix("up")?),
    };
    // Reject e.g. `migrate:downstream`
    (after.is_empty() || after.starts_with(char::is_whitespace)).then_some(marker)
}
