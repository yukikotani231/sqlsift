//! Impact of a schema change on queries (`sqlsift impact`)
//!
//! The query files are analyzed twice, against the schema before and after a
//! change (usually migrations), and the diagnostics are compared: those found
//! only after the change are what it breaks, those found only before are what
//! it fixes.
//!
//! [`SchemaChanges`] lists the tables and views whose definitions differ, so
//! that files that name none of them are not analyzed at all: their
//! diagnostics can't differ.
//!
//! ```
//! use sqlsift_core::impact::{diff_diagnostics, SchemaChanges};
//! use sqlsift_core::schema::SchemaBuilder;
//! use sqlsift_core::Analyzer;
//!
//! let catalog = |sql: &str| {
//!     let mut builder = SchemaBuilder::new();
//!     builder.parse(sql).unwrap();
//!     builder.build().0
//! };
//! let before = catalog("CREATE TABLE users (id INTEGER, email TEXT);");
//! let after = catalog(
//!     "CREATE TABLE users (id INTEGER, email TEXT);
//!      ALTER TABLE users RENAME COLUMN email TO email_address;",
//! );
//!
//! let changes = SchemaChanges::between(&before, &after);
//! let query = "SELECT email FROM users;";
//! assert!(changes.may_affect(query));
//! assert!(!changes.may_affect("SELECT 1 FROM orders;"));
//!
//! let diff = diff_diagnostics(
//!     Analyzer::new(&before).analyze(query),
//!     Analyzer::new(&after).analyze(query),
//! );
//! assert_eq!(diff.introduced.len(), 1);
//! assert!(diff.resolved.is_empty());
//! ```

use serde::Serialize;

use crate::error::Diagnostic;
use crate::schema::Catalog;

/// How a table or view differs between two catalogs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// Only in the catalog after the change
    Added,
    /// Only in the catalog before the change (dropped or renamed)
    Removed,
    /// In both, with a different definition (columns, types, constraints)
    Changed,
}

impl ChangeKind {
    /// Lowercase name, as in JSON output
    pub fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
        }
    }
}

/// A table or view whose definition differs between two catalogs
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ChangedRelation {
    /// Name as `schema.name`
    pub name: String,
    pub kind: ChangeKind,
    /// `"table"` or `"view"`
    pub relation: &'static str,
}

/// The differences between the catalogs before and after a schema change
#[derive(Debug, Clone, Default)]
pub struct SchemaChanges {
    /// Tables and views whose definition differs, sorted by name
    pub relations: Vec<ChangedRelation>,
    /// Something that can affect any query differs (enum types, the search
    /// path, functions the schema defines, skipped definitions, ...): every
    /// query file has to be analyzed
    pub global: bool,
    /// Lowercase names (without schema) of the changed relations
    names: Vec<String>,
}

impl SchemaChanges {
    /// Compare two catalogs
    pub fn between(before: &Catalog, after: &Catalog) -> Self {
        let mut relations = Vec::new();
        for (schema_name, schema) in &after.schemas {
            let old = before.schemas.get(schema_name);
            for (name, table) in &schema.tables {
                let old_table = old.and_then(|s| s.tables.get(name));
                push_change(&mut relations, schema_name, name, "table", old_table, table);
            }
            for (name, view) in &schema.views {
                let old_view = old.and_then(|s| s.views.get(name));
                push_change(&mut relations, schema_name, name, "view", old_view, view);
            }
        }
        for (schema_name, schema) in &before.schemas {
            let new = after.schemas.get(schema_name);
            for name in schema.tables.keys() {
                if !new.is_some_and(|s| s.tables.contains_key(name)) {
                    relations.push(removed(schema_name, name, "table"));
                }
            }
            for name in schema.views.keys() {
                if !new.is_some_and(|s| s.views.contains_key(name)) {
                    relations.push(removed(schema_name, name, "view"));
                }
            }
        }
        relations.sort();

        let mut names: Vec<String> = relations
            .iter()
            .map(|r| unqualified(&r.name).to_lowercase())
            .collect();
        // A dropped or renamed relation's hint ("was renamed to ...") is shown
        // on queries that still name it
        for dropped in &after.dropped_relations {
            if !before.dropped_relations.contains(dropped) {
                names.push(dropped.name.name.to_lowercase());
            }
        }
        names.sort();
        names.dedup();

        Self {
            relations,
            global: globals_differ(before, after),
            names,
        }
    }

    /// Whether nothing differs
    pub fn is_empty(&self) -> bool {
        self.relations.is_empty() && self.names.is_empty() && !self.global
    }

    /// Whether the diagnostics of a query file may differ between the two
    /// catalogs. A file can only see a changed table or view by naming it, so
    /// one whose text contains none of their names (case-insensitively) can't
    /// be affected. This errs on the side of analyzing: a name inside a
    /// comment or a longer word counts too.
    pub fn may_affect(&self, text: &str) -> bool {
        if self.global {
            return true;
        }
        if self.names.is_empty() {
            return false;
        }
        let text = text.to_lowercase();
        self.names.iter().any(|name| text.contains(name.as_str()))
    }
}

/// The name part of `schema.name`
fn unqualified(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, n)| n)
}

fn removed(schema: &str, name: &str, relation: &'static str) -> ChangedRelation {
    ChangedRelation {
        name: format!("{schema}.{name}"),
        kind: ChangeKind::Removed,
        relation,
    }
}

fn push_change<T: Serialize>(
    relations: &mut Vec<ChangedRelation>,
    schema: &str,
    name: &str,
    relation: &'static str,
    before: Option<&T>,
    after: &T,
) {
    let kind = match before {
        None => ChangeKind::Added,
        Some(before) if !same(before, after) => ChangeKind::Changed,
        Some(_) => return,
    };
    relations.push(ChangedRelation {
        name: format!("{schema}.{name}"),
        kind,
        relation,
    });
}

/// Whether two definitions are the same. Definitions are compared through
/// their serialized form, so that every field counts without each type
/// implementing `PartialEq`.
fn same<T: Serialize>(a: &T, b: &T) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(a), Ok(b)) => a == b,
        // Can't tell: treat as changed
        _ => false,
    }
}

/// Whether anything but the tables and views of the catalogs differs
fn globals_differ(before: &Catalog, after: &Catalog) -> bool {
    // Destructured so that a new catalog field has to be considered here
    let Catalog {
        schemas,
        default_schema,
        enums,
        search_path,
        case_sensitive_names,
        strict_schemas,
        skipped_definitions,
        dropped_relations: _,
        databases,
        unknown_unique_keys,
        user_functions,
        user_operators,
    } = before;
    let schema_names_differ = schemas.len() != after.schemas.len()
        || schemas.keys().any(|name| !after.schemas.contains_key(name));
    schema_names_differ
        || *default_schema != after.default_schema
        || !same(enums, &after.enums)
        || *search_path != after.search_path
        || *case_sensitive_names != after.case_sensitive_names
        || *strict_schemas != after.strict_schemas
        || *skipped_definitions != after.skipped_definitions
        || *databases != after.databases
        || *unknown_unique_keys != after.unknown_unique_keys
        || *user_functions != after.user_functions
        || *user_operators != after.user_operators
}

/// The diagnostics of a query file that differ between two catalogs
#[derive(Debug, Clone, Default)]
pub struct DiagnosticDiff {
    /// Found only with the catalog after the change: what it breaks
    pub introduced: Vec<Diagnostic>,
    /// Found only with the catalog before the change: what it fixes
    pub resolved: Vec<Diagnostic>,
}

/// Compare the diagnostics of one query file analyzed against the catalogs
/// before and after a change. The file is the same, so diagnostics are matched
/// by rule and position: one whose message changed (a different type in a
/// type mismatch) is the same problem, neither introduced nor resolved.
pub fn diff_diagnostics(before: Vec<Diagnostic>, after: Vec<Diagnostic>) -> DiagnosticDiff {
    let key = |d: &Diagnostic| (d.kind, d.span.map(|s| (s.offset, s.length)));
    let mut before: Vec<Option<Diagnostic>> = before.into_iter().map(Some).collect();
    let mut introduced = Vec::new();
    let mut unmatched = Vec::new();
    // Exact matches first, so that two problems at one position pair up right
    for diagnostic in after {
        let exact = before.iter().position(|b| {
            b.as_ref()
                .is_some_and(|b| key(b) == key(&diagnostic) && b.message == diagnostic.message)
        });
        match exact {
            Some(i) => before[i] = None,
            None => unmatched.push(diagnostic),
        }
    }
    for diagnostic in unmatched {
        let same_place = before
            .iter()
            .position(|b| b.as_ref().is_some_and(|b| key(b) == key(&diagnostic)));
        match same_place {
            Some(i) => before[i] = None,
            None => introduced.push(diagnostic),
        }
    }
    DiagnosticDiff {
        introduced,
        resolved: before.into_iter().flatten().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaBuilder;
    use crate::Analyzer;

    fn catalog(sql: &str) -> Catalog {
        let mut builder = SchemaBuilder::new();
        builder.parse(sql).unwrap();
        builder.build().0
    }

    const BASE: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);
        CREATE TABLE orders (id INTEGER, user_id INTEGER);";

    fn changes(migration: &str) -> SchemaChanges {
        SchemaChanges::between(&catalog(BASE), &catalog(&format!("{BASE}\n{migration}")))
    }

    fn diff(migration: &str, query: &str) -> DiagnosticDiff {
        let before = catalog(BASE);
        let after = catalog(&format!("{BASE}\n{migration}"));
        diff_diagnostics(
            Analyzer::new(&before).analyze(query),
            Analyzer::new(&after).analyze(query),
        )
    }

    #[test]
    fn no_change() {
        let changes = SchemaChanges::between(&catalog(BASE), &catalog(BASE));
        assert!(changes.is_empty());
        assert!(!changes.may_affect("SELECT * FROM users"));
    }

    #[test]
    fn changed_added_and_removed_relations() {
        let changes = changes(
            "ALTER TABLE users ADD COLUMN name TEXT;
             CREATE TABLE items (id INTEGER);
             DROP TABLE orders;",
        );
        assert!(!changes.global);
        let summary: Vec<(&str, ChangeKind)> = changes
            .relations
            .iter()
            .map(|r| (r.name.as_str(), r.kind))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("public.items", ChangeKind::Added),
                ("public.orders", ChangeKind::Removed),
                ("public.users", ChangeKind::Changed),
            ]
        );
    }

    #[test]
    fn rename_names_both_tables() {
        let changes = changes("ALTER TABLE orders RENAME TO purchases;");
        assert!(changes.may_affect("SELECT id FROM ORDERS"));
        assert!(changes.may_affect("SELECT id FROM purchases"));
        assert!(!changes.may_affect("SELECT id FROM users"));
    }

    #[test]
    fn view_change() {
        let base = format!("{BASE}\nCREATE VIEW v AS SELECT id FROM users;");
        let changes = SchemaChanges::between(
            &catalog(&base),
            &catalog(&format!(
                "{base}\nCREATE OR REPLACE VIEW v AS SELECT id, email FROM users;"
            )),
        );
        assert_eq!(changes.relations.len(), 1);
        assert_eq!(changes.relations[0].relation, "view");
        assert!(changes.may_affect("select * from v"));
    }

    #[test]
    fn enum_change_affects_every_file() {
        let base = "CREATE TYPE mood AS ENUM ('happy');";
        let changes = SchemaChanges::between(
            &catalog(base),
            &catalog(&format!("{base}\nALTER TYPE mood ADD VALUE 'sad';")),
        );
        assert!(changes.global);
        assert!(changes.may_affect("SELECT 1"));
    }

    #[test]
    fn breaks_and_fixes() {
        let query = "SELECT email FROM users;\nSELECT nme FROM users;";
        let diff = diff(
            "ALTER TABLE users RENAME COLUMN email TO email_address;",
            query,
        );
        assert_eq!(diff.introduced.len(), 1);
        assert!(diff.introduced[0].message.contains("email"));
        assert!(diff.resolved.is_empty());

        let diff = diff_diagnostics(
            Analyzer::new(&catalog(&format!(
                "{BASE}\nALTER TABLE users RENAME COLUMN email TO email_address;"
            )))
            .analyze(query),
            Analyzer::new(&catalog(BASE)).analyze(query),
        );
        assert!(diff.introduced.is_empty());
        assert_eq!(diff.resolved.len(), 1);
    }

    #[test]
    fn changed_message_at_same_place_is_not_reported() {
        let diff = diff(
            "ALTER TABLE users ALTER COLUMN email TYPE BOOLEAN;",
            "SELECT 1 FROM users WHERE id = 'x' AND email = 1;",
        );
        // `id = 'x'` is unchanged; `email = 1` turns from text vs integer to
        // boolean vs integer at the same place
        assert!(diff.introduced.is_empty(), "{:?}", diff.introduced);
        assert!(diff.resolved.is_empty(), "{:?}", diff.resolved);
    }

    #[test]
    fn unchanged_diagnostics_are_ignored() {
        let diff = diff(
            "ALTER TABLE users ADD COLUMN name TEXT;",
            "SELECT missing FROM users;",
        );
        assert!(diff.introduced.is_empty());
        assert!(diff.resolved.is_empty());
    }
}
