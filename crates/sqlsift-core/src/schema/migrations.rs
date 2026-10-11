//! Support for migration tool conventions: rollback (down/undo) files and the
//! down sections of dbmate, goose and sql-migrate files.
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

/// Blank out the "down" sections of migration files that hold both directions:
/// every line after a down marker, up to the next up marker (or the end of the
/// file). The markers are those of
///
/// - dbmate: `-- migrate:down` / `-- migrate:up`
/// - goose: `-- +goose Down` / `-- +goose Up`
/// - sql-migrate: `-- +migrate Down` / `-- +migrate Up`
///
/// Removed text is replaced with spaces (newlines are kept), so byte offsets
/// and line numbers of the remaining statements are unchanged. Input without
/// a down marker is returned as is.
///
/// ```
/// use sqlsift_core::schema::strip_down_migrations;
///
/// let sql = "-- +goose Up\nCREATE TABLE t (id INT);\n-- +goose Down\nDROP TABLE t;\n";
/// assert_eq!(
///     strip_down_migrations(sql),
///     "-- +goose Up\nCREATE TABLE t (id INT);\n              \n             \n"
/// );
/// ```
pub fn strip_down_migrations(sql: &str) -> Cow<'_, str> {
    let has_down = sql.contains("migrate:down") || {
        let lower = sql.to_ascii_lowercase();
        lower.contains("+goose down") || lower.contains("+migrate down")
    };
    if !has_down {
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

/// Recognize a marker line: dbmate's `-- migrate:up` or `-- migrate:down`,
/// optionally followed by options such as `transaction:false`, or goose's and
/// sql-migrate's `-- +goose Up` / `-- +migrate Down` (any case, optionally
/// followed by options such as `notransaction`).
fn migrate_marker(line: &str) -> Option<Marker> {
    let rest = line.trim_start().strip_prefix("--")?.trim_start();
    if let Some(rest) = rest.strip_prefix('+') {
        let (tool, rest) = rest.split_once(char::is_whitespace)?;
        if !(tool.eq_ignore_ascii_case("goose") || tool.eq_ignore_ascii_case("migrate")) {
            return None;
        }
        let direction = rest.split_whitespace().next()?;
        return if direction.eq_ignore_ascii_case("up") {
            Some(Marker::Up)
        } else if direction.eq_ignore_ascii_case("down") {
            Some(Marker::Down)
        } else {
            None
        };
    }
    let rest = rest.strip_prefix("migrate:")?;
    let (marker, after) = match rest.strip_prefix("down") {
        Some(after) => (Marker::Down, after),
        None => (Marker::Up, rest.strip_prefix("up")?),
    };
    // Reject e.g. `migrate:downstream`
    (after.is_empty() || after.starts_with(char::is_whitespace)).then_some(marker)
}
