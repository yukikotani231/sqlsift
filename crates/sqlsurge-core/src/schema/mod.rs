//! Schema management module

mod builder;
mod catalog;

pub use builder::SchemaBuilder;
pub use catalog::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, IdentityKind,
    PrimaryKeyDef, QualifiedName, Schema, TableDef, UniqueConstraintDef, ViewDef,
};
