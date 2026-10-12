//! Schema definition statements.

use crate::{DataType, Expr, Ident, ObjectName, OrderByExpr, Query};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreateTable {
    pub or_replace: bool,
    pub temporary: bool,
    pub if_not_exists: bool,
    pub name: ObjectName,
    pub columns: Vec<ColumnDef>,
    pub constraints: Vec<TableConstraint>,
    /// Hive `PARTITIONED BY (c TYPE)`
    pub hive_distribution: HiveDistributionStyle,
    pub query: Option<Box<Query>>,
    pub like: Option<CreateTableLikeKind>,
    /// `CREATE TABLE t CLONE s` (Snowflake, BigQuery)
    pub clone: Option<ObjectName>,
    /// PostgreSQL `INHERITS (p)`
    pub inherits: Option<Vec<ObjectName>>,
    /// PostgreSQL `PARTITION OF p`
    pub partition_of: Option<ObjectName>,
    /// The expressions of the clauses the analyzer doesn't model (`PARTITION BY`, ...)
    pub other: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HiveDistributionStyle {
    PARTITIONED { columns: Vec<ColumnDef> },
    NONE,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CreateTableLikeKind {
    Parenthesized(CreateTableLike),
    Plain(CreateTableLike),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreateTableLike {
    pub name: ObjectName,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnDef {
    pub name: Ident,
    pub data_type: DataType,
    pub options: Vec<ColumnOptionDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnOptionDef {
    pub name: Option<Ident>,
    pub option: ColumnOption,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ColumnOption {
    Null,
    NotNull,
    Default(Expr),
    PrimaryKey(PrimaryKeyConstraint),
    Unique(UniqueConstraint),
    ForeignKey(ForeignKeyConstraint),
    Check(CheckConstraint),
    /// Dialect-specific words (MySQL `AUTO_INCREMENT`): the tokens' text
    DialectSpecific(Vec<String>),
    OnUpdate(Expr),
    Generated {
        generated_as: GeneratedAs,
        generation_expr: Option<Expr>,
    },
    /// `IDENTITY` / `AUTOINCREMENT` (Snowflake, T-SQL)
    Identity,
    /// Any other option: its text and expressions
    Other(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratedAs {
    Always,
    ByDefault,
    ExpStored,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TableConstraint {
    Unique(UniqueConstraint),
    PrimaryKey(PrimaryKeyConstraint),
    ForeignKey(ForeignKeyConstraint),
    Check(CheckConstraint),
    /// MySQL `INDEX` / `KEY`
    Index(IndexConstraint),
    /// MySQL `FULLTEXT` / `SPATIAL`
    FulltextOrSpatial(IndexConstraint),
    PrimaryKeyUsingIndex(ConstraintUsingIndex),
    UniqueUsingIndex(ConstraintUsingIndex),
    /// PostgreSQL `EXCLUDE`
    Exclude(ExcludeConstraint),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UniqueConstraint {
    pub name: Option<Ident>,
    pub index_name: Option<Ident>,
    pub columns: Vec<IndexColumn>,
    pub nulls_distinct: NullsDistinctOption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NullsDistinctOption {
    None,
    Distinct,
    NotDistinct,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrimaryKeyConstraint {
    pub name: Option<Ident>,
    pub index_name: Option<Ident>,
    pub columns: Vec<IndexColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForeignKeyConstraint {
    pub name: Option<Ident>,
    pub index_name: Option<Ident>,
    pub columns: Vec<Ident>,
    pub foreign_table: ObjectName,
    pub referred_columns: Vec<Ident>,
    pub on_delete: Option<ReferentialAction>,
    pub on_update: Option<ReferentialAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferentialAction {
    Restrict,
    Cascade,
    SetNull,
    NoAction,
    SetDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CheckConstraint {
    pub name: Option<Ident>,
    pub expr: Box<Expr>,
    pub no_inherit: bool,
    pub enforced: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexConstraint {
    pub name: Option<Ident>,
    pub columns: Vec<IndexColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConstraintUsingIndex {
    pub name: Option<Ident>,
    pub index_name: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExcludeConstraint {
    pub name: Option<Ident>,
    pub elements: Vec<IndexColumn>,
    pub predicate: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndexColumn {
    pub column: OrderByExpr,
    pub operator_class: Option<ObjectName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreateIndex {
    pub name: Option<ObjectName>,
    pub table_name: ObjectName,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    pub if_not_exists: bool,
    pub include: Vec<Ident>,
    pub nulls_distinct: Option<bool>,
    pub predicate: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreateView {
    pub or_replace: bool,
    pub materialized: bool,
    pub name: ObjectName,
    pub columns: Vec<ViewColumnDef>,
    pub query: Box<Query>,
    pub if_not_exists: bool,
    pub temporary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewColumnDef {
    pub name: Ident,
    pub data_type: Option<DataType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UserDefinedTypeRepresentation {
    Composite {
        attributes: Vec<UserDefinedTypeCompositeAttributeDef>,
    },
    Enum {
        labels: Vec<Ident>,
    },
    /// `AS RANGE (...)`
    Range,
    /// `CREATE TYPE t (...)`
    SqlDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserDefinedTypeCompositeAttributeDef {
    pub name: Ident,
    pub data_type: DataType,
    pub collation: Option<ObjectName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlterType {
    pub name: ObjectName,
    pub operation: AlterTypeOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlterTypeOperation {
    Rename(AlterTypeRename),
    AddValue(AlterTypeAddValue),
    RenameValue(AlterTypeRenameValue),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlterTypeRename {
    pub new_name: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlterTypeAddValue {
    pub if_not_exists: bool,
    pub value: Ident,
    pub position: Option<AlterTypeAddValuePosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlterTypeAddValuePosition {
    Before(Ident),
    After(Ident),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlterTypeRenameValue {
    pub from: Ident,
    pub to: Ident,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlterTable {
    pub name: ObjectName,
    pub if_exists: bool,
    pub only: bool,
    pub operations: Vec<AlterTableOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlterTableOperation {
    AddConstraint {
        constraint: TableConstraint,
        not_valid: bool,
    },
    AddColumn {
        column_keyword: bool,
        if_not_exists: bool,
        column_def: ColumnDef,
    },
    DropConstraint {
        if_exists: bool,
        name: Ident,
    },
    DropColumn {
        has_column_keyword: bool,
        column_names: Vec<Ident>,
        if_exists: bool,
    },
    DropPrimaryKey,
    RenameColumn {
        old_column_name: Ident,
        new_column_name: Ident,
    },
    RenameTable {
        table_name: RenameTableNameKind,
    },
    ChangeColumn {
        old_name: Ident,
        new_name: Ident,
        data_type: DataType,
        options: Vec<ColumnOption>,
    },
    ModifyColumn {
        col_name: Ident,
        data_type: DataType,
        options: Vec<ColumnOption>,
    },
    RenameConstraint {
        old_name: Ident,
        new_name: Ident,
    },
    AlterColumn {
        column_name: Ident,
        op: AlterColumnOperation,
    },
    /// Any other operation: its text and expressions
    Other(String, Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenameTableNameKind {
    As(ObjectName),
    To(ObjectName),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlterColumnOperation {
    SetNotNull,
    DropNotNull,
    SetDefault {
        value: Expr,
    },
    DropDefault,
    SetDataType {
        data_type: DataType,
        using: Option<Expr>,
        had_set: bool,
    },
    AddGenerated {
        generated_as: Option<GeneratedAs>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CreateTrigger {
    pub name: ObjectName,
    pub period: Option<TriggerPeriod>,
    pub events: Vec<TriggerEvent>,
    pub table_name: ObjectName,
    pub condition: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerPeriod {
    For,
    After,
    Before,
    InsteadOf,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TriggerEvent {
    Insert,
    Update(Vec<Ident>),
    Delete,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectType {
    Collation,
    Table,
    View,
    MaterializedView,
    Index,
    Schema,
    Database,
    Role,
    Sequence,
    Stage,
    Type,
    User,
    Stream,
    Warehouse,
    Other,
}
