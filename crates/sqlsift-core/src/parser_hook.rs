//! Experimental (#257): the in-house PostgreSQL parser (`crates/sqlsift-parser`).
//!
//! Built in with the `inhouse-parser` feature and switched on with [`set_enabled`] (the CLI
//! does it for `SQLSIFT_PARSER=inhouse`), so the analyzer and the schema builder can run
//! on its output and their diagnostics be compared with sqlparser's. Until then, nothing
//! changes.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether PostgreSQL text is read with the in-house parser, for the whole process.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// The 1-based line and column (in characters) of byte `offset` of `text`.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (line, before[line_start..].chars().count() + 1)
}
