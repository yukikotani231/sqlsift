//! Schema management module

mod builder;
mod catalog;

pub use builder::SchemaBuilder;
pub use catalog::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, FormerColumn,
    IdentityKind, PrimaryKeyDef, QualifiedName, Schema, SkippedDefinition, TableDef,
    UniqueConstraintDef, ViewDef,
};
