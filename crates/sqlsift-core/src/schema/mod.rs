//! Schema management module

mod builder;
mod catalog;
mod migrations;

pub use builder::SchemaBuilder;
pub(crate) use builder::{mask_unsupported_clauses, unparsed_definition};
pub use catalog::{
    Catalog, CheckConstraintDef, ColumnDef, DefaultValue, EnumTypeDef, ForeignKeyDef, FormerColumn,
    IdentityKind, PrimaryKeyDef, QualifiedName, Schema, SkippedDefinition, TableDef,
    UniqueConstraintDef, ViewDef,
};
pub use migrations::{is_rollback_migration, strip_down_migrations};
