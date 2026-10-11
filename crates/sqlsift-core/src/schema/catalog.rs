//! Schema catalog - stores table and column definitions

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sqlparser::ast::{Ident, ObjectName};
use std::borrow::Cow;

use crate::types::SqlType;

/// Schema catalog - holds all table/view information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// Schema name -> Schema
    pub schemas: IndexMap<String, Schema>,
    /// Default schema name (e.g., "public" for PostgreSQL)
    pub default_schema: String,
    /// Enum type definitions (name, `schema.name` when created with a schema ->
    /// EnumTypeDef)
    pub enums: IndexMap<String, EnumTypeDef>,
    /// Schemas searched, in order, for unqualified names (PostgreSQL
    /// `SET search_path`). Empty means just [`Self::default_schema`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_path: Vec<String>,
    /// PostgreSQL identifier rules: unquoted names fold to lowercase and quoted names
    /// are case-sensitive. When false, table/view/schema names match case-insensitively.
    #[serde(default)]
    pub case_sensitive_names: bool,
    /// Schemas are namespaces with a known search path (PostgreSQL): a name is
    /// looked up only in the schema it names, or in the search path. When false
    /// (MySQL and SQLite databases, where the current database is chosen at run
    /// time and schema files often leave it unnamed), a name that is not found
    /// there falls back to the default schema (qualified names) or to any schema
    /// (unqualified names), so that queries are not flagged for a database the
    /// schema input does not name.
    #[serde(default)]
    pub strict_schemas: bool,
    /// Table, view and type definitions in the schema input that could not be parsed
    /// and were skipped (for diagnostics on queries that use them)
    #[serde(default)]
    pub skipped_definitions: Vec<SkippedDefinition>,
    /// Tables and views dropped earlier in the query file being analyzed (for
    /// diagnostics on queries that reference them)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_relations: Vec<DroppedRelation>,
    /// Databases (BigQuery projects) named by three-part names of tables and views
    /// in the schema input (`project.dataset.table`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub databases: Vec<String>,
    /// A statement of the schema input that may define a unique index or
    /// constraint could not be parsed, so the unique keys of tables are not fully
    /// known
    #[serde(default)]
    pub unknown_unique_keys: bool,
}

/// A schema statement that could not be parsed and was skipped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedDefinition {
    /// Kind of statement (`CREATE TABLE`, `CREATE VIEW`, `ALTER TABLE`, ...)
    pub kind: String,
    /// Name of the object it defines or alters, if it could be determined
    pub name: Option<String>,
    /// Line of the statement when it is in the query file being analyzed (`None`
    /// for the schema input)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// A table or view dropped earlier in the query file being analyzed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedRelation {
    pub name: QualifiedName,
    pub line: usize,
}

impl Catalog {
    pub fn new() -> Self {
        let mut catalog = Self {
            schemas: IndexMap::new(),
            default_schema: "public".to_string(),
            enums: IndexMap::new(),
            search_path: Vec::new(),
            case_sensitive_names: false,
            strict_schemas: false,
            skipped_definitions: Vec::new(),
            dropped_relations: Vec::new(),
            databases: Vec::new(),
            unknown_unique_keys: false,
        };
        // Create default schema
        catalog.schemas.insert(
            "public".to_string(),
            Schema {
                name: "public".to_string(),
                tables: IndexMap::new(),
                views: IndexMap::new(),
            },
        );
        catalog
    }

    /// Get or create a schema
    pub fn get_or_create_schema(&mut self, name: &str) -> &mut Schema {
        self.schemas
            .entry(name.to_string())
            .or_insert_with(|| Schema {
                name: name.to_string(),
                tables: IndexMap::new(),
                views: IndexMap::new(),
            })
    }

    /// Add a table to the catalog
    pub fn add_table(&mut self, table: TableDef) {
        let schema_name = table
            .name
            .schema
            .clone()
            .unwrap_or_else(|| self.creation_schema().to_string());
        let schema = self.get_or_create_schema(&schema_name);
        schema.tables.insert(table.name.name.clone(), table);
    }

    /// Look up a table by name
    ///
    /// Names are matched exactly first, then case-insensitively (unquoted
    /// identifiers are case-insensitive in SQL).
    pub fn get_table(&self, name: &QualifiedName) -> Option<&TableDef> {
        let (schema, table) = self.locate(name, |s| &s.tables)?;
        self.schemas[schema].tables.get_index(table).map(|(_, t)| t)
    }

    /// Look up a table by name (mutable)
    pub fn get_table_mut(&mut self, name: &QualifiedName) -> Option<&mut TableDef> {
        let (schema, table) = self.locate(name, |s| &s.tables)?;
        let (_, schema) = self.schemas.get_index_mut(schema)?;
        schema.tables.get_index_mut(table).map(|(_, t)| t)
    }

    /// Schemas searched for unqualified names, in order: the search path, or the
    /// default schema
    pub fn search_path(&self) -> impl Iterator<Item = &str> {
        let default = self
            .search_path
            .is_empty()
            .then_some(self.default_schema.as_str());
        self.search_path.iter().map(String::as_str).chain(default)
    }

    /// Schema that unqualified objects are created in: the first schema of the
    /// search path, or the default schema
    pub fn creation_schema(&self) -> &str {
        self.search_path.first().unwrap_or(&self.default_schema)
    }

    /// Index of the schema and of the object named `name` in it, among the objects
    /// of each schema given by `objects` (an unqualified name is looked up in the
    /// schemas of the search path, in order; see [`Self::strict_schemas`] for the
    /// fallbacks of non-strict catalogs)
    fn locate<V>(
        &self,
        name: &QualifiedName,
        objects: impl Fn(&Schema) -> &IndexMap<String, V>,
    ) -> Option<(usize, usize)> {
        let find_in = |schema: usize| {
            let object = self.index_of(objects(&self.schemas[schema]), &name.name)?;
            Some((schema, object))
        };
        let find = |schema_name: &str| find_in(self.index_of(&self.schemas, schema_name)?);
        let found = self.locate_exact(name, &objects);
        if found.is_some() || self.strict_schemas {
            return found;
        }
        match &name.schema {
            // `db.table` where the schema input left the database unnamed
            Some(_) => self.search_path().find_map(find),
            // The current database is chosen at run time: any database may have it
            None => (0..self.schemas.len()).find_map(find_in),
        }
    }

    /// [`Self::locate`] without the fallbacks of non-strict catalogs: in the schema
    /// `name` names, or in the search path
    fn locate_exact<V>(
        &self,
        name: &QualifiedName,
        objects: impl Fn(&Schema) -> &IndexMap<String, V>,
    ) -> Option<(usize, usize)> {
        let find = |schema_name: &str| {
            let schema = self.index_of(&self.schemas, schema_name)?;
            let object = self.index_of(objects(&self.schemas[schema]), &name.name)?;
            Some((schema, object))
        };
        match &name.schema {
            Some(schema_name) => find(schema_name),
            None => self.search_path().find_map(find),
        }
    }

    /// Whether a table or view `name` exists exactly where `CREATE ... name` would
    /// create it (for `IF NOT EXISTS`): no fallback to other schemas
    pub fn relation_exists_exactly(&self, name: &QualifiedName) -> bool {
        let name = QualifiedName {
            schema: Some(
                name.schema
                    .clone()
                    .unwrap_or_else(|| self.creation_schema().to_string()),
            ),
            name: name.name.clone(),
        };
        self.locate_exact(&name, |s| &s.tables).is_some()
            || self.locate_exact(&name, |s| &s.views).is_some()
    }

    /// Fully qualified name (schema and object name as stored in the catalog) of
    /// the table or view `name` refers to
    pub fn resolve_relation(&self, name: &QualifiedName) -> Option<QualifiedName> {
        let full = |schema: usize, object: Option<&String>| {
            let schema_name = self.schemas.get_index(schema)?.0;
            Some(QualifiedName::with_schema(
                schema_name.clone(),
                object?.clone(),
            ))
        };
        if let Some((schema, table)) = self.locate(name, |s| &s.tables) {
            let key = self.schemas[schema].tables.get_index(table).map(|(k, _)| k);
            return full(schema, key);
        }
        let (schema, view) = self.locate(name, |s| &s.views)?;
        let key = self.schemas[schema].views.get_index(view).map(|(k, _)| k);
        full(schema, key)
    }

    /// A table or view named `name` (unqualified) in a schema that is not on the
    /// search path, as a schema-qualified name. Only for catalogs with
    /// [`Self::strict_schemas`], where such a name is not found unqualified.
    pub fn relation_off_search_path(&self, name: &str) -> Option<QualifiedName> {
        if !self.strict_schemas {
            return None;
        }
        self.schemas
            .iter()
            .filter(|(schema, _)| !self.search_path().any(|s| self.names_match(s, schema)))
            .find_map(|(schema_name, schema)| {
                let key = schema
                    .tables
                    .keys()
                    .chain(schema.views.keys())
                    .find(|k| self.names_match(k, name))?;
                Some(QualifiedName::with_schema(schema_name.clone(), key.clone()))
            })
    }

    /// Name of an identifier as stored in the catalog: with PostgreSQL rules, unquoted
    /// identifiers fold to lowercase and quoted ones are kept as written
    pub fn ident_name(&self, ident: &Ident) -> String {
        if self.case_sensitive_names && ident.quote_style.is_none() {
            ident.value.to_lowercase()
        } else {
            ident.value.clone()
        }
    }

    /// Convert a (possibly schema-qualified) object name to a [`QualifiedName`],
    /// applying [`Self::ident_name`] to each part
    ///
    /// A three-part name is `database.schema.table`: the database is not modeled
    /// and is ignored. A backtick-quoted identifier containing dots
    /// (`` `project.dataset.table` ``) is split into its parts.
    pub fn qualified_name(&self, name: &ObjectName) -> QualifiedName {
        match split_dotted_idents(&name.0).as_ref() {
            [table] => QualifiedName::new(self.ident_name(table)),
            [schema, table] | [_, schema, table] => {
                QualifiedName::with_schema(self.ident_name(schema), self.ident_name(table))
            }
            _ => QualifiedName::new(name.to_string()),
        }
    }

    /// Record the database (BigQuery project) of a three-part table or view name
    pub fn note_database(&mut self, name: &ObjectName) {
        if let [database, _, _] = split_dotted_idents(&name.0).as_ref() {
            let database = self.ident_name(database);
            if !self
                .databases
                .iter()
                .any(|d| self.names_match(d, &database))
            {
                self.databases.push(database);
            }
        }
    }

    /// Whether a three-part name's database may be one the schema input describes:
    /// one that is named by a table or view of the schema, or any database when the
    /// schema names none. Names with fewer parts always may.
    pub fn database_is_known(&self, name: &ObjectName) -> bool {
        match split_dotted_idents(&name.0).as_ref() {
            [database, _, _] => {
                self.databases.is_empty()
                    || self
                        .databases
                        .iter()
                        .any(|d| self.names_match(d, &self.ident_name(database)))
            }
            _ => true,
        }
    }

    /// Whether two catalog names refer to the same object
    pub fn names_match(&self, a: &str, b: &str) -> bool {
        if self.case_sensitive_names {
            a == b
        } else {
            a.eq_ignore_ascii_case(b)
        }
    }

    /// Whether two qualified names refer to the same relation
    pub fn relation_names_match(&self, a: &QualifiedName, b: &QualifiedName) -> bool {
        if !self.names_match(&a.name, &b.name) {
            return false;
        }
        match (&a.schema, &b.schema) {
            (Some(sa), Some(sb)) => self.names_match(sa, sb),
            (Some(s), None) | (None, Some(s)) => {
                self.names_match(s, &self.default_schema)
                    || self.search_path.iter().any(|sp| self.names_match(s, sp))
            }
            (None, None) => true,
        }
    }

    /// Record a table or view dropped earlier in the file being analyzed
    pub fn record_dropped_relation(&mut self, name: QualifiedName, line: usize) {
        self.dropped_relations.push(DroppedRelation { name, line });
    }

    /// Remove any recorded drops that match `name`
    pub fn restore_relation(&mut self, name: &QualifiedName) {
        for i in (0..self.dropped_relations.len()).rev() {
            if self.relation_names_match(&self.dropped_relations[i].name, name) {
                self.dropped_relations.swap_remove(i);
            }
        }
    }

    /// Index of `key` in `map` following the catalog's case rules
    fn index_of<V>(&self, map: &IndexMap<String, V>, key: &str) -> Option<usize> {
        if self.case_sensitive_names {
            map.get_index_of(key)
        } else {
            index_ignore_case(map, key)
        }
    }

    /// Check if a table exists
    pub fn table_exists(&self, name: &QualifiedName) -> bool {
        self.get_table(name).is_some()
    }

    /// Add an enum type to the catalog (replacing one with the same name)
    pub fn add_enum(&mut self, enum_def: EnumTypeDef) {
        let key = enum_def.qualified_name();
        let schema = enum_def
            .schema
            .clone()
            .unwrap_or_else(|| self.default_schema.clone());
        match self.enum_index(&format!("{schema}.{}", enum_def.name)) {
            Some(index) => {
                self.enums.shift_remove_index(index);
                self.enums.shift_insert(index, key, enum_def);
            }
            None => {
                self.enums.insert(key, enum_def);
            }
        }
    }

    /// Get an enum type by name: `name`, or `schema.name` (as in a column type such
    /// as `public.mood`). An unqualified name is looked up in the search path, then
    /// in any schema.
    pub fn get_enum(&self, name: &str) -> Option<&EnumTypeDef> {
        self.enum_index(name)
            .and_then(|i| self.enums.get_index(i).map(|(_, e)| e))
    }

    /// Index in [`Self::enums`] of the enum type `name` (see [`Self::get_enum`])
    fn enum_index(&self, name: &str) -> Option<usize> {
        let (schema, base) = match name.rsplit_once('.') {
            // `db.schema.type`: the schema is the second-to-last part
            Some((schema, base)) => (Some(schema.rsplit('.').next().unwrap_or(schema)), base),
            None => (None, name),
        };
        let default_schema = self.default_schema.as_str();
        // Exact names first, then ignoring case
        let find = |schema: Option<&str>| {
            self.enums
                .values()
                .position(|e| {
                    e.name == base
                        && schema
                            .map_or(true, |s| e.schema.as_deref().unwrap_or(default_schema) == s)
                })
                .or_else(|| {
                    self.enums.values().position(|e| {
                        e.name.eq_ignore_ascii_case(base)
                            && schema.map_or(true, |s| {
                                e.schema
                                    .as_deref()
                                    .unwrap_or(default_schema)
                                    .eq_ignore_ascii_case(s)
                            })
                    })
                })
        };
        match schema {
            Some(schema) => find(Some(schema)),
            None => self
                .search_path()
                .find_map(|s| find(Some(s)))
                .or_else(|| find(None)),
        }
    }

    /// Check if an enum type exists
    pub fn enum_exists(&self, name: &str) -> bool {
        self.get_enum(name).is_some()
    }

    /// Get an enum type by name (mutable)
    pub fn get_enum_mut(&mut self, name: &str) -> Option<&mut EnumTypeDef> {
        let index = self.enum_index(name)?;
        self.enums.get_index_mut(index).map(|(_, e)| e)
    }

    /// Drop an enum type from the catalog
    pub fn drop_enum(&mut self, name: &str) {
        if let Some(index) = self.enum_index(name) {
            self.enums.shift_remove_index(index);
        }
    }

    /// Drop a view from the catalog
    pub fn drop_view(&mut self, name: &QualifiedName) {
        if let Some((schema, view)) = self.locate(name, |s| &s.views) {
            self.schemas[schema].views.shift_remove_index(view);
        }
    }

    /// Rename a table, keeping it in its schema
    pub fn rename_table(&mut self, name: &QualifiedName, new_name: String) {
        let Some((schema, index)) = self.locate(name, |s| &s.tables) else {
            return;
        };
        let tables = &mut self.schemas[schema].tables;
        if let Some((_, mut table)) = tables.shift_remove_index(index) {
            table.name.name.clone_from(&new_name);
            tables.insert(new_name, table);
        }
    }

    /// Drop a table from the catalog
    pub fn drop_table(&mut self, name: &QualifiedName) {
        if let Some((schema, table)) = self.locate(name, |s| &s.tables) {
            self.schemas[schema].tables.shift_remove_index(table);
        }
    }

    /// Add a view to the catalog
    pub fn add_view(&mut self, view: ViewDef) {
        let schema_name = view
            .name
            .schema
            .clone()
            .unwrap_or_else(|| self.creation_schema().to_string());
        let schema = self.get_or_create_schema(&schema_name);
        schema.views.insert(view.name.name.clone(), view);
    }

    /// Look up a view by name
    pub fn get_view(&self, name: &QualifiedName) -> Option<&ViewDef> {
        let (schema, view) = self.locate(name, |s| &s.views)?;
        self.schemas[schema].views.get_index(view).map(|(_, v)| v)
    }

    /// Check if a view exists
    pub fn view_exists(&self, name: &QualifiedName) -> bool {
        self.get_view(name).is_some()
    }

    /// Get all table names
    pub fn table_names(&self) -> Vec<QualifiedName> {
        self.schemas
            .iter()
            .flat_map(|(schema_name, schema)| {
                schema.tables.keys().map(move |table_name| QualifiedName {
                    schema: Some(schema_name.clone()),
                    name: table_name.clone(),
                })
            })
            .collect()
    }

    /// Get all table and view names (for typo suggestions)
    pub fn table_or_view_names(&self) -> Vec<QualifiedName> {
        self.schemas
            .iter()
            .flat_map(|(schema_name, schema)| {
                let tables = schema.tables.keys().map(move |name| QualifiedName {
                    schema: Some(schema_name.clone()),
                    name: name.clone(),
                });
                let views = schema.views.keys().map(move |name| QualifiedName {
                    schema: Some(schema_name.clone()),
                    name: name.clone(),
                });
                tables.chain(views)
            })
            .collect()
    }
}

/// The parts of a dotted name, with each backtick-quoted identifier that contains
/// dots (`` `project.dataset.table` ``, a single identifier for the parser) split
/// into its parts. Double-quoted identifiers are kept whole: in PostgreSQL `"a.b"`
/// is one name.
pub fn split_dotted_idents(parts: &[Ident]) -> Cow<'_, [Ident]> {
    let splits = |ident: &Ident| ident.quote_style == Some('`') && ident.value.contains('.');
    if !parts.iter().any(splits) {
        return Cow::Borrowed(parts);
    }
    let mut split = Vec::with_capacity(parts.len() + 2);
    for ident in parts {
        if splits(ident) {
            split.extend(ident.value.split('.').map(|value| Ident {
                value: value.to_string(),
                quote_style: ident.quote_style,
                span: ident.span,
            }));
        } else {
            split.push(ident.clone());
        }
    }
    Cow::Owned(split)
}

/// Index of `key` in `map`, matching exactly first and then ignoring ASCII case
fn index_ignore_case<V>(map: &IndexMap<String, V>, key: &str) -> Option<usize> {
    map.get_index_of(key)
        .or_else(|| map.keys().position(|k| k.eq_ignore_ascii_case(key)))
}

/// A database schema (namespace)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub tables: IndexMap<String, TableDef>,
    pub views: IndexMap<String, ViewDef>,
}

/// Qualified name (schema.table or just table)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QualifiedName {
    pub schema: Option<String>,
    pub name: String,
}

impl QualifiedName {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: None,
            name: name.into(),
        }
    }

    pub fn with_schema(schema: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            schema: Some(schema.into()),
            name: name.into(),
        }
    }

    /// Parse from a dotted name like "schema.table" or just "table" (in
    /// "database.schema.table" the database is ignored)
    pub fn parse(s: &str) -> Self {
        if let Some((qualifier, name)) = s.rsplit_once('.') {
            let schema = qualifier.rsplit('.').next().unwrap_or(qualifier);
            Self::with_schema(schema, name)
        } else {
            Self::new(s)
        }
    }
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(schema) = &self.schema {
            write!(f, "{}.{}", schema, self.name)
        } else {
            write!(f, "{}", self.name)
        }
    }
}

/// Table definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDef {
    pub name: QualifiedName,
    pub columns: IndexMap<String, ColumnDef>,
    pub primary_key: Option<PrimaryKeyDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub unique_constraints: Vec<UniqueConstraintDef>,
    pub check_constraints: Vec<CheckConstraintDef>,
    /// Columns the table no longer has because ALTER TABLE renamed or dropped them
    /// (for diagnostics on queries that still use the old names)
    #[serde(default)]
    pub former_columns: Vec<FormerColumn>,
    /// The table may have unique indexes sqlsift doesn't know: partial or
    /// expression unique indexes, or indexes a partition gets from its parent
    #[serde(default)]
    pub unknown_unique_keys: bool,
}

/// A column that ALTER TABLE renamed or dropped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormerColumn {
    /// The old column name
    pub name: String,
    /// The column's current name if it was renamed, or `None` if it was dropped
    pub renamed_to: Option<String>,
}

impl TableDef {
    pub fn new(name: QualifiedName) -> Self {
        Self {
            name,
            columns: IndexMap::new(),
            primary_key: None,
            foreign_keys: Vec::new(),
            unique_constraints: Vec::new(),
            check_constraints: Vec::new(),
            former_columns: Vec::new(),
            unknown_unique_keys: false,
        }
    }

    /// The renamed or dropped column that was called `name`, if any
    pub fn former_column(&self, name: &str) -> Option<&FormerColumn> {
        self.former_columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Remember that column `old` was renamed to `new`. Earlier names of `old` now
    /// refer to `new` as well.
    pub fn record_column_rename(&mut self, old: &str, new: &str) {
        if old.eq_ignore_ascii_case(new) {
            return;
        }
        self.forget_former_column(new);
        self.forget_former_column(old);
        for former in &mut self.former_columns {
            if former
                .renamed_to
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(old))
            {
                former.renamed_to = Some(new.to_string());
            }
        }
        self.former_columns.push(FormerColumn {
            name: old.to_string(),
            renamed_to: Some(new.to_string()),
        });
    }

    /// Remember that column `name` was dropped. Earlier names of it are dropped too.
    pub fn record_column_drop(&mut self, name: &str) {
        self.forget_former_column(name);
        for former in &mut self.former_columns {
            if former
                .renamed_to
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
            {
                former.renamed_to = None;
            }
        }
        self.former_columns.push(FormerColumn {
            name: name.to_string(),
            renamed_to: None,
        });
    }

    /// Forget a former column name (a column with that name exists again)
    pub fn forget_former_column(&mut self, name: &str) {
        self.former_columns
            .retain(|c| !c.name.eq_ignore_ascii_case(name));
    }

    /// Get a column by name
    pub fn get_column(&self, name: &str) -> Option<&ColumnDef> {
        // Case-insensitive lookup
        self.columns
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    /// Check if a column exists
    pub fn column_exists(&self, name: &str) -> bool {
        self.get_column(name).is_some()
    }

    /// Get all column names
    pub fn column_names(&self) -> Vec<&str> {
        self.columns
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }
}

/// Column definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // independent facts about a column
pub struct ColumnDef {
    pub name: String,
    pub data_type: SqlType,
    pub nullable: bool,
    pub default: Option<DefaultValue>,
    pub is_primary_key: bool,
    pub identity: Option<IdentityKind>,
    /// MySQL AUTO_INCREMENT / SQLite AUTOINCREMENT: a value is generated when omitted
    #[serde(default)]
    pub auto_increment: bool,
    /// A generated (computed) column, `GENERATED ALWAYS AS (expr)`: its value can't
    /// be given by INSERT or UPDATE
    #[serde(default)]
    pub generated: bool,
}

impl ColumnDef {
    pub fn new(name: impl Into<String>, data_type: SqlType) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable: true,
            default: None,
            is_primary_key: false,
            identity: None,
            auto_increment: false,
            generated: false,
        }
    }

    #[must_use]
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    #[must_use]
    pub fn with_default(mut self, default: DefaultValue) -> Self {
        self.default = Some(default);
        self
    }

    #[must_use]
    pub fn primary_key(mut self) -> Self {
        self.is_primary_key = true;
        self.nullable = false;
        self
    }
}

/// Default value for a column
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DefaultValue {
    Literal(String),
    Expression(String),
    CurrentTimestamp,
    Null,
    NextVal(String), // For sequences/SERIAL
}

/// Primary key constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimaryKeyDef {
    pub name: Option<String>,
    pub columns: Vec<String>,
}

/// Foreign key constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyDef {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub references_table: QualifiedName,
    pub references_columns: Vec<String>,
}

/// Unique constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueConstraintDef {
    pub name: Option<String>,
    pub columns: Vec<String>,
}

/// CHECK constraint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConstraintDef {
    pub name: Option<String>,
    pub expression: String,
}

/// Enum type definition (CREATE TYPE ... AS ENUM)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumTypeDef {
    pub name: String,
    pub values: Vec<String>,
    /// Schema the type was created in, when its name was qualified
    /// (`CREATE TYPE billing.state AS ENUM ...`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
}

impl EnumTypeDef {
    /// `schema.name`, or `name` if the type was created without a schema
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Identity column kind (GENERATED ... AS IDENTITY)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IdentityKind {
    Always,
    ByDefault,
}

/// View definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewDef {
    pub name: QualifiedName,
    /// Column names (empty if they couldn't be determined)
    pub columns: Vec<String>,
    /// Column types inferred from the view's query, parallel to `columns`
    /// (`SqlType::Unknown` where the type couldn't be inferred)
    #[serde(default)]
    pub column_types: Vec<SqlType>,
    pub materialized: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualified_name_parse() {
        let name = QualifiedName::parse("users");
        assert_eq!(name.schema, None);
        assert_eq!(name.name, "users");

        let name = QualifiedName::parse("public.users");
        assert_eq!(name.schema, Some("public".to_string()));
        assert_eq!(name.name, "users");
    }

    #[test]
    fn test_catalog_add_table() {
        let mut catalog = Catalog::new();
        let table = TableDef::new(QualifiedName::new("users"));
        catalog.add_table(table);

        assert!(catalog.table_exists(&QualifiedName::new("users")));
        assert!(catalog.table_exists(&QualifiedName::with_schema("public", "users")));
    }
}
