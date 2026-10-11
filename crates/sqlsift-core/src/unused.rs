//! `sqlsift unused`: tables and columns that no query uses
//!
//! [`Usage`] gathers the [`Reference`]s that the analyzer records for every
//! query of a project (and for the schema's views), and [`Usage::unused`] lists
//! the catalog tables and columns they never touch.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::analyzer::{Reference, ReferenceKind};
use crate::schema::{Catalog, QualifiedName, TableDef};

/// How the queries use the catalog's tables and columns. Names are lowercased
/// unless the catalog is case-sensitive.
#[derive(Debug, Default)]
pub struct Usage {
    /// Tables and views used in any way (`schema.table`)
    relations: HashSet<String>,
    /// Tables read with `*`
    all_columns: HashSet<String>,
    /// Columns read (`schema.table`, `column`)
    read: HashSet<(String, String)>,
    /// Columns written, with the statements (file, line) writing them
    written: BTreeMap<(String, String), BTreeSet<(String, usize)>>,
}

/// A table or column no query uses
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Unused {
    /// The table: its name, qualified when it is not in the default schema
    pub table: String,
    /// The column, or `None` when the whole table is unused
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Number of statements writing the column (0: never referenced)
    pub writes: usize,
}

impl Unused {
    /// `table` or `table.column`
    pub fn name(&self) -> String {
        match &self.column {
            Some(column) => format!("{}.{column}", self.table),
            None => self.table.clone(),
        }
    }
}

impl Usage {
    /// Add the references of one file (or of the schema's views)
    pub fn add(&mut self, file: &str, references: &[Reference], catalog: &Catalog) {
        for reference in references {
            let relation = key(catalog, &reference.relation.to_string());
            match &reference.kind {
                ReferenceKind::Relation => {}
                ReferenceKind::AllColumns => {
                    self.all_columns.insert(relation.clone());
                }
                ReferenceKind::Column(column) => {
                    let column = (relation.clone(), key(catalog, column));
                    if reference.write {
                        self.written
                            .entry(column)
                            .or_default()
                            .insert((file.to_string(), reference.span.line));
                    } else {
                        self.read.insert(column);
                    }
                }
            }
            self.relations.insert(relation);
        }
    }

    /// The tables no query uses, then the columns of used tables that no query
    /// reads, in catalog order. Columns of keys, foreign keys and CHECK
    /// constraints count as used.
    pub fn unused(&self, catalog: &Catalog) -> Vec<Unused> {
        let mut tables = Vec::new();
        let mut columns = Vec::new();
        for schema in catalog.schemas.values() {
            for table in schema.tables.values() {
                let relation = key(catalog, &qualified(catalog, &table.name));
                let name = display_name(catalog, &table.name);
                if !self.relations.contains(&relation) {
                    tables.push(Unused {
                        table: name,
                        column: None,
                        writes: 0,
                    });
                    continue;
                }
                if self.all_columns.contains(&relation) {
                    continue;
                }
                let constrained = constrained_columns(catalog, table);
                for column in table.columns.values() {
                    let column_key = (relation.clone(), key(catalog, &column.name));
                    if self.read.contains(&column_key) || constrained.contains(&column_key.1) {
                        continue;
                    }
                    columns.push(Unused {
                        table: name.clone(),
                        column: Some(column.name.clone()),
                        writes: self.written.get(&column_key).map_or(0, BTreeSet::len),
                    });
                }
            }
        }
        tables.extend(columns);
        tables
    }
}

/// The schema-qualified name of a table, as references name it
fn qualified(catalog: &Catalog, name: &QualifiedName) -> String {
    catalog
        .resolve_relation(name)
        .map_or_else(|| name.to_string(), |q| q.to_string())
}

/// The name of a table for display: without the default schema
fn display_name(catalog: &Catalog, name: &QualifiedName) -> String {
    let resolved = catalog.resolve_relation(name);
    match resolved {
        Some(q) if q.schema.as_deref() == Some(catalog.default_schema.as_str()) => q.name,
        Some(q) => q.to_string(),
        None => name.to_string(),
    }
}

fn key(catalog: &Catalog, name: &str) -> String {
    if catalog.case_sensitive_names {
        name.to_string()
    } else {
        name.to_lowercase()
    }
}

/// Columns of `table` that its constraints use (primary key, unique keys,
/// foreign keys in either direction, CHECK constraints), as keys
fn constrained_columns(catalog: &Catalog, table: &TableDef) -> HashSet<String> {
    let mut columns: HashSet<String> = HashSet::new();
    let mut add = |names: &[String]| {
        columns.extend(names.iter().map(|c| key(catalog, c)));
    };
    if let Some(pk) = &table.primary_key {
        add(&pk.columns);
    }
    for unique in &table.unique_constraints {
        add(&unique.columns);
    }
    for fk in &table.foreign_keys {
        add(&fk.columns);
    }
    // Columns other tables' foreign keys reference
    let name = key(catalog, &qualified(catalog, &table.name));
    for schema in catalog.schemas.values() {
        for other in schema.tables.values() {
            for fk in &other.foreign_keys {
                if key(catalog, &qualified(catalog, &fk.references_table)) == name {
                    add(&fk.references_columns);
                }
            }
        }
    }
    // CHECK constraints: every word of the expression that names a column
    let words: HashSet<String> = table
        .check_constraints
        .iter()
        .flat_map(|check| {
            check
                .expression
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .map(|w| key(catalog, w))
                .collect::<Vec<_>>()
        })
        .collect();
    for column in table.columns.values() {
        let column = key(catalog, &column.name);
        if words.contains(&column) {
            columns.insert(column);
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::Analyzer;
    use crate::schema::SchemaBuilder;

    fn unused(schema: &str, queries: &str) -> Vec<String> {
        let mut builder = SchemaBuilder::new();
        builder.parse(schema).expect("schema parses");
        let catalog = builder.build().0;
        let mut usage = Usage::default();
        let mut analyzer = Analyzer::new(&catalog).with_references();
        analyzer.analyze(queries);
        usage.add("q.sql", analyzer.references(), &catalog);
        usage
            .unused(&catalog)
            .iter()
            .map(|u| format!("{} {}", u.name(), u.writes))
            .collect()
    }

    #[test]
    fn tables_and_columns() {
        let schema =
            "CREATE TABLE users (id INT PRIMARY KEY, name TEXT, token TEXT, seen_at TIMESTAMP);
                      CREATE TABLE audit_log (id INT, message TEXT);";
        assert_eq!(
            unused(
                schema,
                "SELECT name FROM users;
                 UPDATE users SET seen_at = now() WHERE id = 1;
                 INSERT INTO users (id, name, seen_at) VALUES (2, 'b', now());"
            ),
            ["audit_log 0", "users.token 0", "users.seen_at 2"]
        );
    }

    #[test]
    fn star_reads_every_column() {
        let schema = "CREATE TABLE users (id INT, name TEXT);";
        assert!(unused(schema, "SELECT * FROM users").is_empty());
    }

    #[test]
    fn insert_without_column_list_writes_every_column() {
        let schema = "CREATE TABLE users (id INT PRIMARY KEY, name TEXT);";
        assert_eq!(
            unused(schema, "INSERT INTO users VALUES (1, 'a')"),
            ["users.name 1"]
        );
    }

    #[test]
    fn constraint_columns_count_as_used() {
        let schema = "CREATE TABLE users (id INT PRIMARY KEY, email TEXT UNIQUE, age INT CHECK (age > 0));
                      CREATE TABLE orders (id INT PRIMARY KEY, user_id INT REFERENCES users(id), note TEXT);";
        assert_eq!(
            unused(schema, "SELECT id FROM orders; SELECT id FROM users;"),
            ["orders.note 0"]
        );
    }

    #[test]
    fn views_read_their_columns() {
        let schema = "CREATE TABLE users (id INT, name TEXT, token TEXT);
                      CREATE VIEW names AS SELECT id, name FROM users;";
        assert_eq!(unused(schema, schema), ["users.token 0"]);
    }
}
