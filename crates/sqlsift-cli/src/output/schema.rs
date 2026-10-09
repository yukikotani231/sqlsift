//! Output of `sqlsift schema`: the catalog sqlsift built from the schema files.
//!
//! Objects are listed in definition order (schema files are read in the order
//! given, and directories in filename order), so the output is deterministic.

use std::path::PathBuf;

use serde_json::{json, Value};
use sqlsift_core::schema::{Catalog, ColumnDef, DefaultValue, IdentityKind, TableDef, ViewDef};
use sqlsift_core::types::SqlType;
use sqlsift_core::SqlDialect;

use crate::args::SchemaFormat;

/// What `sqlsift schema` prints
pub struct SchemaReport<'a> {
    pub catalog: &'a Catalog,
    pub dialect: SqlDialect,
    pub schema_files: &'a [PathBuf],
}

impl SchemaReport<'_> {
    /// Render the report in the given format
    pub fn render(&self, format: SchemaFormat) -> String {
        match format {
            SchemaFormat::Human => self.to_human(),
            SchemaFormat::Json => {
                let mut out = serde_json::to_string_pretty(&self.to_json())
                    .expect("schema JSON is serializable");
                out.push('\n');
                out
            }
        }
    }

    fn to_human(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let _ = writeln!(out, "Schema Information:");
        let _ = writeln!(out, "==================");
        let _ = writeln!(out, "Dialect: {}", self.dialect);
        let _ = writeln!(out, "Schema files:");
        for file in self.schema_files {
            let _ = writeln!(out, "  {}", file.display());
        }

        for (schema_name, schema) in &self.catalog.schemas {
            let _ = writeln!(out, "\nSchema: {schema_name}");
            if schema.tables.is_empty() && schema.views.is_empty() {
                let _ = writeln!(out, "  (no tables or views)");
            }
            for (table_name, table) in &schema.tables {
                let _ = writeln!(out, "  Table: {table_name}");
                for col in table.columns.values() {
                    let _ = writeln!(out, "    - {}", column_line(col));
                }
                for fk in &table.foreign_keys {
                    let _ = writeln!(
                        out,
                        "    foreign key ({}) references {} ({})",
                        fk.columns.join(", "),
                        fk.references_table,
                        fk.references_columns.join(", ")
                    );
                }
            }
            for (view_name, view) in &schema.views {
                let kind = if view.materialized {
                    "Materialized view"
                } else {
                    "View"
                };
                let _ = writeln!(out, "  {kind}: {view_name}");
                if view.columns.is_empty() {
                    let _ = writeln!(out, "    (columns unknown)");
                }
                for (name, ty) in view_columns(view) {
                    match ty {
                        Some(ty) => {
                            let _ = writeln!(out, "    - {} {}", name, ty.display_name());
                        }
                        None => {
                            let _ = writeln!(out, "    - {name}");
                        }
                    }
                }
            }
        }

        if !self.catalog.enums.is_empty() {
            let _ = writeln!(out, "\nEnum types:");
            for (name, def) in &self.catalog.enums {
                let values: Vec<String> = def.values.iter().map(|v| format!("'{v}'")).collect();
                let _ = writeln!(out, "  {}: {}", name, values.join(", "));
            }
        }
        out
    }

    fn to_json(&self) -> Value {
        let schemas: Vec<Value> = self
            .catalog
            .schemas
            .iter()
            .map(|(name, schema)| {
                json!({
                    "name": name,
                    "tables": schema.tables.values().map(table_json).collect::<Vec<_>>(),
                    "views": schema.views.values().map(view_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        let enums: Vec<Value> = self
            .catalog
            .enums
            .values()
            .map(|e| json!({ "name": e.name, "schema": e.schema, "values": e.values }))
            .collect();
        json!({
            "dialect": self.dialect.to_string(),
            "default_schema": self.catalog.default_schema,
            "schema_files": self
                .schema_files
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "schemas": schemas,
            "enums": enums,
        })
    }
}

/// `name type NULL|NOT NULL [PRIMARY KEY] [identity] [AUTO_INCREMENT] [DEFAULT ...]`
fn column_line(col: &ColumnDef) -> String {
    let mut line = format!(
        "{} {} {}",
        col.name,
        col.data_type.display_name(),
        if col.nullable { "NULL" } else { "NOT NULL" }
    );
    if col.is_primary_key {
        line.push_str(" PRIMARY KEY");
    }
    if let Some(identity) = identity_name(col) {
        line.push(' ');
        line.push_str(identity);
    }
    if col.auto_increment {
        line.push_str(" AUTO_INCREMENT");
    }
    if let Some(default) = default_sql(col) {
        line.push_str(" DEFAULT ");
        line.push_str(&default);
    }
    line
}

fn identity_name(col: &ColumnDef) -> Option<&'static str> {
    col.identity.as_ref().map(|kind| match kind {
        IdentityKind::Always => "GENERATED ALWAYS AS IDENTITY",
        IdentityKind::ByDefault => "GENERATED BY DEFAULT AS IDENTITY",
    })
}

fn default_sql(col: &ColumnDef) -> Option<String> {
    col.default.as_ref().map(|d| match d {
        DefaultValue::Literal(s) | DefaultValue::Expression(s) | DefaultValue::NextVal(s) => {
            s.clone()
        }
        DefaultValue::CurrentTimestamp => "CURRENT_TIMESTAMP".to_string(),
        DefaultValue::Null => "NULL".to_string(),
    })
}

/// A view's columns with their types (`None` where the type couldn't be inferred)
fn view_columns(view: &ViewDef) -> impl Iterator<Item = (&str, Option<&SqlType>)> {
    view.columns.iter().enumerate().map(|(i, name)| {
        let ty = view
            .column_types
            .get(i)
            .filter(|t| !matches!(t, SqlType::Unknown));
        (name.as_str(), ty)
    })
}

fn table_json(table: &TableDef) -> Value {
    let columns: Vec<Value> = table
        .columns
        .values()
        .map(|col| {
            json!({
                "name": col.name,
                "type": col.data_type.display_name(),
                "nullable": col.nullable,
                "primary_key": col.is_primary_key,
                "identity": identity_name(col),
                "auto_increment": col.auto_increment,
                "default": default_sql(col),
            })
        })
        .collect();
    let foreign_keys: Vec<Value> = table
        .foreign_keys
        .iter()
        .map(|fk| {
            json!({
                "name": fk.name,
                "columns": fk.columns,
                "references_table": fk.references_table.to_string(),
                "references_columns": fk.references_columns,
            })
        })
        .collect();
    let unique: Vec<&Vec<String>> = table
        .unique_constraints
        .iter()
        .map(|u| &u.columns)
        .collect();
    json!({
        "name": table.name.name,
        "columns": columns,
        "primary_key": table.primary_key.as_ref().map(|pk| &pk.columns),
        "foreign_keys": foreign_keys,
        "unique": unique,
    })
}

fn view_json(view: &ViewDef) -> Value {
    let columns: Vec<Value> = view_columns(view)
        .map(|(name, ty)| json!({ "name": name, "type": ty.map(SqlType::display_name) }))
        .collect();
    json!({
        "name": view.name.name,
        "materialized": view.materialized,
        "columns": columns,
    })
}
