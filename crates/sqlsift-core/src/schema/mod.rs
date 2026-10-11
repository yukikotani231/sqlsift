//! Schema management module

mod builder;
mod catalog;
mod migrations;
mod warehouse;

pub use builder::SchemaBuilder;
pub(crate) use builder::{mask_unsupported_clauses, unparsed_definition};
pub(crate) use catalog::split_name;
pub use catalog::{
    case_mismatched_column, split_dotted_idents, Catalog, CheckConstraintDef, ColumnDef,
    DefaultValue, DroppedRelation, EnumTypeDef, ForeignKeyDef, FormerColumn, IdentityKind,
    PrimaryKeyDef, QualifiedName, Schema, SkippedDefinition, SourceLocation, TableDef,
    UniqueConstraintDef, ViewDef,
};
pub use migrations::{is_rollback_migration, strip_down_migrations};
