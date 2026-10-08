//! Schema management module

mod builder;
mod catalog;
mod migrations;

pub use builder::SchemaBuilder;
pub use catalog::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, IdentityKind,
    PrimaryKeyDef, QualifiedName, Schema, TableDef, UniqueConstraintDef, ViewDef,
};
pub use migrations::{is_rollback_migration, strip_down_migrations};
