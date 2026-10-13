//! Statements, and the data-modifying ones.

use crate::{
    AlterTable, AlterType, AttachedToken, CreateIndex, CreateTable, CreateTrigger, CreateView,
    DataType, Expr, Function, Ident, ObjectName, ObjectType, OrderByExpr, Query, SelectItem, Span,
    TableFactor, TableWithJoins, UserDefinedTypeRepresentation,
};

/// A statement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::large_enum_variant)]
pub enum Statement {
    Query(Box<Query>),
    Insert(Insert),
    Update(Update),
    Delete(Delete),
    Merge(Merge),
    CreateTable(CreateTable),
    CreateView(CreateView),
    CreateIndex(CreateIndex),
    CreateType {
        name: ObjectName,
        representation: Option<UserDefinedTypeRepresentation>,
    },
    CreateTrigger(CreateTrigger),
    AlterTable(AlterTable),
    AlterType(AlterType),
    Drop {
        object_type: ObjectType,
        if_exists: bool,
        names: Vec<ObjectName>,
        cascade: bool,
        temporary: bool,
    },
    Set(Set),
    Use(Use),
    /// `DECLARE` (BigQuery scripting variables, cursors)
    Declare {
        stmts: Vec<Declare>,
    },
    /// A statement the analyzer doesn't model: its text, location and the expressions and
    /// queries in it
    Other(OtherStatement),
}

/// A statement the analyzer doesn't model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OtherStatement {
    pub text: String,
    pub span: Span,
    pub exprs: Vec<Expr>,
    pub queries: Vec<Query>,
    /// The tables it names (`TRUNCATE t`, `COPY t`)
    pub relations: Vec<ObjectName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Insert {
    pub insert_token: AttachedToken,
    /// MySQL `INSERT IGNORE`
    pub ignore: bool,
    pub into: bool,
    pub table: TableObject,
    pub table_alias: Option<TableAliasWithoutColumns>,
    pub columns: Vec<ObjectName>,
    pub overwrite: bool,
    pub source: Option<Box<Query>>,
    /// MySQL `INSERT ... SET a = 1`
    pub assignments: Vec<Assignment>,
    pub partitioned: Option<Vec<Expr>>,
    pub after_columns: Vec<Ident>,
    pub has_table_keyword: bool,
    pub on: Option<OnInsert>,
    pub returning: Option<Vec<SelectItem>>,
    pub replace_into: bool,
    /// SQLite `INSERT OR REPLACE` and the like
    pub or: bool,
    /// MySQL `INSERT ... AS new(a, b)`: the alias of the inserted row
    pub insert_alias: Option<InsertAliases>,
}

/// `INSERT INTO t AS alias` (PostgreSQL)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableAliasWithoutColumns {
    pub explicit: bool,
    pub alias: Ident,
}

/// MySQL `INSERT ... AS new(a, b)`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InsertAliases {
    pub row_alias: ObjectName,
    pub col_aliases: Option<Vec<Ident>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TableObject {
    TableName(ObjectName),
    TableFunction(Function),
    TableQuery(Box<Query>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OnInsert {
    DuplicateKeyUpdate(Vec<Assignment>),
    OnConflict(OnConflict),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OnConflict {
    pub conflict_target: Option<ConflictTarget>,
    pub action: OnConflictAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConflictTarget {
    Columns(Vec<Ident>),
    OnConstraint(ObjectName),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OnConflictAction {
    DoNothing,
    DoUpdate(DoUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DoUpdate {
    pub assignments: Vec<Assignment>,
    pub selection: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Assignment {
    pub target: AssignmentTarget,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssignmentTarget {
    ColumnName(ObjectName),
    Tuple(Vec<ObjectName>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Update {
    pub update_token: AttachedToken,
    pub table: TableWithJoins,
    pub assignments: Vec<Assignment>,
    pub from: Option<UpdateTableFromKind>,
    pub selection: Option<Expr>,
    pub returning: Option<Vec<SelectItem>>,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UpdateTableFromKind {
    BeforeSet(Vec<TableWithJoins>),
    AfterSet(Vec<TableWithJoins>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Delete {
    pub delete_token: AttachedToken,
    pub tables: Vec<ObjectName>,
    pub from: FromTable,
    pub using: Option<Vec<TableWithJoins>>,
    pub selection: Option<Expr>,
    pub returning: Option<Vec<SelectItem>>,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FromTable {
    WithFromKeyword(Vec<TableWithJoins>),
    WithoutKeyword(Vec<TableWithJoins>),
}

/// `MERGE INTO target USING source ON ...`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Merge {
    pub table: TableFactor,
    pub source: TableFactor,
    pub on: Box<Expr>,
    /// The expressions of the WHEN clauses
    pub clauses: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Set {
    SingleAssignment {
        variable: ObjectName,
        values: Vec<Expr>,
    },
    /// Any other SET statement
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Use {
    Catalog(ObjectName),
    Schema(ObjectName),
    Database(ObjectName),
    Warehouse(ObjectName),
    Role(ObjectName),
    Object(ObjectName),
    Default,
    SecondaryRoles,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Declare {
    pub names: Vec<Ident>,
    pub data_type: Option<DataType>,
    /// `DEFAULT expr` / `= expr`
    pub assignment: Option<Expr>,
    pub for_query: Option<Box<Query>>,
}
