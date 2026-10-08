//! Schema catalog - stores table and column definitions

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sqlparser::ast::{Ident, ObjectName};

use crate::types::SqlType;

/// Schema catalog - holds all table/view information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// Schema name -> Schema
    pub schemas: IndexMap<String, Schema>,
    /// Default schema name (e.g., "public" for PostgreSQL)
    pub default_schema: String,
    /// Enum type definitions (name -> EnumTypeDef)
    pub enums: IndexMap<String, EnumTypeDef>,
    /// PostgreSQL identifier rules: unquoted names fold to lowercase and quoted names
    /// are case-sensitive. When false, table/view/schema names match case-insensitively.
    #[serde(default)]
    pub case_sensitive_names: bool,
}

impl Catalog {
    pub fn new() -> Self {
        let mut catalog = Self {
            schemas: IndexMap::new(),
            default_schema: "public".to_string(),
            enums: IndexMap::new(),
            case_sensitive_names: false,
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
        if !self.schemas.contains_key(name) {
            self.schemas.insert(
                name.to_string(),
                Schema {
                    name: name.to_string(),
                    tables: IndexMap::new(),
                    views: IndexMap::new(),
                },
            );
        }
        self.schemas
            .get_mut(name)
            .expect("schema was just inserted")
    }

    /// Add a table to the catalog
    pub fn add_table(&mut self, table: TableDef) {
        let schema_name = table
            .name
            .schema
            .clone()
            .unwrap_or_else(|| self.default_schema.clone());
        let schema = self.get_or_create_schema(&schema_name);
        schema.tables.insert(table.name.name.clone(), table);
    }

    /// Look up a table by name
    ///
    /// Names are matched exactly first, then case-insensitively (unquoted
    /// identifiers are case-insensitive in SQL).
    pub fn get_table(&self, name: &QualifiedName) -> Option<&TableDef> {
        let schema = self.get_schema(name)?;
        self.lookup(&schema.tables, &name.name)
    }

    /// Look up a table by name (mutable)
    pub fn get_table_mut(&mut self, name: &QualifiedName) -> Option<&mut TableDef> {
        let schema_name = name.schema.as_ref().unwrap_or(&self.default_schema);
        let schema_index = self.index_of(&self.schemas, schema_name)?;
        let table_index = self.index_of(&self.schemas[schema_index].tables, &name.name)?;
        let (_, schema) = self.schemas.get_index_mut(schema_index)?;
        schema.tables.get_index_mut(table_index).map(|(_, t)| t)
    }

    /// Look up the schema a (possibly unqualified) name refers to
    fn get_schema(&self, name: &QualifiedName) -> Option<&Schema> {
        let schema_name = name.schema.as_ref().unwrap_or(&self.default_schema);
        self.lookup(&self.schemas, schema_name)
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
    pub fn qualified_name(&self, name: &ObjectName) -> QualifiedName {
        match name.0.as_slice() {
            [table] => QualifiedName::new(self.ident_name(table)),
            [schema, table] | [_, schema, table] => {
                QualifiedName::with_schema(self.ident_name(schema), self.ident_name(table))
            }
            _ => QualifiedName::new(name.to_string()),
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

    /// Look up `key` in `map` following the catalog's case rules
    fn lookup<'m, V>(&self, map: &'m IndexMap<String, V>, key: &str) -> Option<&'m V> {
        self.index_of(map, key)
            .and_then(|i| map.get_index(i).map(|(_, v)| v))
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

    /// Add an enum type to the catalog
    pub fn add_enum(&mut self, enum_def: EnumTypeDef) {
        self.enums.insert(enum_def.name.clone(), enum_def);
    }

    /// Get an enum type by name
    pub fn get_enum(&self, name: &str) -> Option<&EnumTypeDef> {
        get_ignore_case(&self.enums, name)
    }

    /// Check if an enum type exists
    pub fn enum_exists(&self, name: &str) -> bool {
        self.get_enum(name).is_some()
    }

    /// Get an enum type by name (mutable)
    pub fn get_enum_mut(&mut self, name: &str) -> Option<&mut EnumTypeDef> {
        let index = index_ignore_case(&self.enums, name)?;
        self.enums.get_index_mut(index).map(|(_, e)| e)
    }

    /// Drop an enum type from the catalog
    pub fn drop_enum(&mut self, name: &str) {
        if let Some(index) = index_ignore_case(&self.enums, name) {
            self.enums.shift_remove_index(index);
        }
    }

    /// Drop a view from the catalog
    pub fn drop_view(&mut self, name: &QualifiedName) {
        let schema_name = name.schema.as_ref().unwrap_or(&self.default_schema);
        let Some(schema_index) = self.index_of(&self.schemas, schema_name) else {
            return;
        };
        if let Some(index) = self.index_of(&self.schemas[schema_index].views, &name.name) {
            self.schemas[schema_index].views.shift_remove_index(index);
        }
    }

    /// Drop a table from the catalog
    pub fn drop_table(&mut self, name: &QualifiedName) {
        let schema_name = name.schema.as_ref().unwrap_or(&self.default_schema);
        let Some(schema_index) = self.index_of(&self.schemas, schema_name) else {
            return;
        };
        if let Some(index) = self.index_of(&self.schemas[schema_index].tables, &name.name) {
            self.schemas[schema_index].tables.shift_remove_index(index);
        }
    }

    /// Add a view to the catalog
    pub fn add_view(&mut self, view: ViewDef) {
        let schema_name = view
            .name
            .schema
            .clone()
            .unwrap_or_else(|| self.default_schema.clone());
        let schema = self.get_or_create_schema(&schema_name);
        schema.views.insert(view.name.name.clone(), view);
    }

    /// Look up a view by name
    pub fn get_view(&self, name: &QualifiedName) -> Option<&ViewDef> {
        let schema = self.get_schema(name)?;
        self.lookup(&schema.views, &name.name)
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

/// Index of `key` in `map`, matching exactly first and then ignoring ASCII case
fn index_ignore_case<V>(map: &IndexMap<String, V>, key: &str) -> Option<usize> {
    map.get_index_of(key)
        .or_else(|| map.keys().position(|k| k.eq_ignore_ascii_case(key)))
}

/// Look up `key` in `map`, matching exactly first and then ignoring ASCII case
fn get_ignore_case<'m, V>(map: &'m IndexMap<String, V>, key: &str) -> Option<&'m V> {
    index_ignore_case(map, key).and_then(|i| map.get_index(i).map(|(_, v)| v))
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

    /// Parse from a dotted name like "schema.table" or just "table"
    pub fn parse(s: &str) -> Self {
        if let Some((schema, name)) = s.split_once('.') {
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
        }
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
        self.columns.keys().map(|s| s.as_str()).collect()
    }
}

/// Column definition
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
    }

    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    pub fn with_default(mut self, default: DefaultValue) -> Self {
        self.default = Some(default);
        self
    }

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
