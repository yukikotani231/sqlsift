//! Query scopes: the relations (tables, views, CTEs, subqueries) visible at each
//! point of a statement, with the names and types of their columns.
//!
//! A [`Scope`] is a stack of [`Frame`]s, one per query block. Name resolution and
//! type inference both look names up through it, so they share one model of SQL's
//! visibility rules:
//!
//! - a subquery in an expression sees the enclosing query blocks (correlation)
//! - a non-LATERAL subquery in FROM doesn't see the FROM clause it appears in
//! - CTEs are visible in the query that defines them and everything nested in it

use indexmap::IndexMap;
use sqlparser::ast::Ident;
use std::collections::HashSet;

use crate::dialect::SqlDialect;
use crate::schema::{case_mismatched_column, Catalog, ColumnDef, FormerColumn, QualifiedName};
use crate::types::SqlType;

/// Type of an expression or column, as far as it can be inferred
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExpressionType {
    /// Type is known (successfully inferred)
    Known(SqlType),
    /// Quoted string literal: untyped until it meets another operand
    /// (e.g. `'2024-01-01'` compared with a DATE column is a DATE)
    StringLiteral(String),
    /// Type is unknown (e.g. NULL, an unsupported function)
    Unknown,
}

impl ExpressionType {
    /// Expression type of a column (or cast) with the given declared type. Columns
    /// whose type sqlsift doesn't model (typeless SQLite columns, unsupported types,
    /// semi-structured types such as Snowflake's VARIANT) are unknown.
    pub(super) fn of_column(data_type: &SqlType) -> Self {
        match data_type {
            SqlType::Unknown | SqlType::SemiStructured(_) => ExpressionType::Unknown,
            t => ExpressionType::Known(t.clone()),
        }
    }

    /// The SQL type a column of this expression type gets (string literals are text)
    pub(super) fn to_sql_type(&self) -> SqlType {
        match self {
            ExpressionType::Known(t) => t.clone(),
            ExpressionType::StringLiteral(_) => SqlType::Text,
            ExpressionType::Unknown => SqlType::Unknown,
        }
    }
}

/// An output column of a relation
#[derive(Debug, Clone)]
pub(super) struct Column {
    pub(super) name: String,
    pub(super) ty: ExpressionType,
    /// The name was guessed from an unaliased expression (`count(*)` -> `count`),
    /// which dialects name differently: it is good enough to resolve references,
    /// but not to say two columns have the same name
    pub(super) guessed_name: bool,
    /// A catalog table column created with a quoted, not all lowercase name
    /// (PostgreSQL `"authorId"`): only a quoted reference with the same case
    /// matches it (see [`ColumnDef::case_sensitive`](crate::schema::ColumnDef))
    pub(super) case_sensitive: bool,
    /// Whether the column may be NULL (`None` if that can't be told)
    pub(super) nullable: Option<bool>,
}

/// A column of unknown type and NULL-ability (a system column such as `ctid`)
static UNKNOWN_COLUMN: Column = Column {
    name: String::new(),
    ty: ExpressionType::Unknown,
    guessed_name: false,
    case_sensitive: false,
    nullable: None,
};

impl Column {
    pub(super) fn new(name: impl Into<String>, ty: ExpressionType) -> Self {
        Self {
            name: name.into(),
            ty,
            guessed_name: false,
            case_sensitive: false,
            nullable: None,
        }
    }

    /// Give the column a name it certainly has (an alias or a column list)
    pub(super) fn rename(&mut self, name: &str) {
        name.clone_into(&mut self.name);
        self.guessed_name = false;
    }
}

/// Name PostgreSQL gives an output column it can't name; sqlsift also uses it for
/// columns whose name it can't determine, so they match any name
pub(super) const UNNAMED_COLUMN: &str = "?column?";

/// What a relation in FROM is (used in diagnostics)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RelationKind {
    Table,
    View,
    Cte,
    Subquery,
    Function,
    /// A table that doesn't exist, or a system catalog: columns are unknown
    Opaque,
}

/// A relation visible in a query block
#[derive(Debug, Clone)]
pub(super) struct Relation {
    pub(super) kind: RelationKind,
    /// Name used in diagnostics (table, view or CTE name, or the alias)
    pub(super) name: String,
    /// Output columns, or `None` if they can't be determined (then any column may
    /// belong to the relation)
    pub(super) columns: Option<Vec<Column>>,
    /// Columns merged into an earlier relation by `JOIN ... USING` / `NATURAL JOIN`
    /// (lowercase): they appear once in `SELECT *`
    pub(super) merged: HashSet<String>,
    /// Columns of a catalog table that ALTER TABLE renamed or dropped
    pub(super) former_columns: Vec<FormerColumn>,
    /// Schema-qualified name of a catalog table or view referenced without an
    /// alias (`schema.table.column` refers to it); `None` for anything else
    pub(super) qualified: Option<QualifiedName>,
    /// The catalog table or view the relation is (aliased or not), for
    /// [`Reference`](super::Reference)s; `None` for anything else
    pub(super) source: Option<QualifiedName>,
}

/// Whether a relation has a column
pub(super) enum ColumnMatch<'r> {
    /// The relation has the column
    Yes(&'r Column),
    /// The relation's columns aren't fully known, so it may have the column
    Maybe,
    /// The relation has several columns with this name (`SELECT *` over a join in a
    /// CTE or subquery), so a reference to it is ambiguous
    Duplicate,
    No,
}

impl Relation {
    pub(super) fn new(
        kind: RelationKind,
        name: impl Into<String>,
        columns: Option<Vec<Column>>,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            columns,
            merged: HashSet::new(),
            former_columns: Vec::new(),
            qualified: None,
            source: None,
        }
    }

    /// A catalog table, with its declared column types
    pub(super) fn table(
        catalog: &Catalog,
        dialect: SqlDialect,
        name: &QualifiedName,
    ) -> Option<Self> {
        let table = catalog.get_table(name)?;
        // SQLite lets a PRIMARY KEY column other than INTEGER PRIMARY KEY hold NULL
        let nullable = |c: &ColumnDef| {
            c.nullable
                || dialect == SqlDialect::SQLite
                    && c.is_primary_key
                    && c.data_type != SqlType::Integer
        };
        let columns = table
            .columns
            .values()
            .map(|c| Column {
                case_sensitive: c.case_sensitive,
                nullable: Some(nullable(c)),
                ..Column::new(c.name.clone(), ExpressionType::of_column(&c.data_type))
            })
            .collect();
        let mut relation = Self::new(RelationKind::Table, name.to_string(), Some(columns));
        relation.former_columns.clone_from(&table.former_columns);
        relation.qualified = catalog.resolve_relation(name);
        relation.source.clone_from(&relation.qualified);
        Some(relation)
    }

    /// What the relation is, as a word for diagnostics (`table`, `view`, `CTE`, ...)
    pub(super) fn kind_name(&self) -> &'static str {
        match self.kind {
            RelationKind::Table => "table",
            RelationKind::View => "view",
            RelationKind::Cte => "CTE",
            RelationKind::Function => "function",
            _ => "subquery",
        }
    }

    /// The renamed or dropped column of this table that was called `name`
    pub(super) fn former_column(&self, name: &str) -> Option<&FormerColumn> {
        self.former_columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// A catalog view, with the column types inferred from its query
    pub(super) fn view(catalog: &Catalog, name: &QualifiedName) -> Option<Self> {
        let view = catalog.get_view(name)?;
        // An empty column list means the view's columns couldn't be determined
        let columns = (!view.columns.is_empty()).then(|| {
            view.columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let ty = view
                        .column_types
                        .get(i)
                        .map_or(ExpressionType::Unknown, ExpressionType::of_column);
                    Column::new(c.clone(), ty)
                })
                .collect()
        });
        let mut relation = Self::new(RelationKind::View, name.to_string(), columns);
        relation.qualified = catalog.resolve_relation(name);
        relation.source.clone_from(&relation.qualified);
        Some(relation)
    }

    /// Apply an explicit column list (`AS t(a, b)`, `WITH t(a, b)`): it renames the
    /// leading columns, and defines the columns of a relation whose columns are unknown
    pub(super) fn rename_columns(
        columns: Option<Vec<Column>>,
        names: &[String],
    ) -> Option<Vec<Column>> {
        if names.is_empty() {
            return columns;
        }
        let mut columns = columns.unwrap_or_default();
        for (i, name) in names.iter().enumerate() {
            match columns.get_mut(i) {
                Some(col) => col.rename(name),
                None => columns.push(Column::new(name.clone(), ExpressionType::Unknown)),
            }
        }
        Some(columns)
    }

    /// Look up a column of this relation
    pub(super) fn column(&self, name: &str, dialect: SqlDialect) -> ColumnMatch<'_> {
        let Some(columns) = &self.columns else {
            return ColumnMatch::Maybe;
        };
        if let Some(col) = columns.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
            if dialect.rejects_duplicate_column_references()
                && !col.guessed_name
                && columns
                    .iter()
                    .filter(|c| !c.guessed_name && c.name == col.name)
                    .nth(1)
                    .is_some()
            {
                return ColumnMatch::Duplicate;
            }
            return ColumnMatch::Yes(col);
        }
        if self.kind == RelationKind::Table && is_system_column(dialect, name) {
            return ColumnMatch::Yes(&UNKNOWN_COLUMN);
        }
        if columns.iter().any(|c| c.name == UNNAMED_COLUMN) {
            return ColumnMatch::Maybe;
        }
        ColumnMatch::No
    }

    /// The case-sensitive column that a reference matches only ignoring case
    /// (see [`case_mismatched_column`])
    pub(super) fn case_mismatched_column(&self, ident: &Ident) -> Option<&str> {
        case_mismatched_column(
            self.columns
                .iter()
                .flatten()
                .map(|c| (c.name.as_str(), c.case_sensitive)),
            ident,
        )
    }

    /// Column names, for "did you mean" suggestions
    pub(super) fn column_names(&self) -> impl Iterator<Item = String> + '_ {
        self.columns
            .iter()
            .flatten()
            .filter(|c| c.name != UNNAMED_COLUMN)
            .map(|c| c.name.clone())
    }

    /// Columns produced by `SELECT *` over this relation
    pub(super) fn star_columns(&self) -> Option<impl Iterator<Item = &Column>> {
        let columns = self.columns.as_ref()?;
        Some(
            columns
                .iter()
                .filter(|c| !self.merged.contains(&c.name.to_lowercase())),
        )
    }
}

/// A CTE visible in a query
#[derive(Debug, Clone)]
pub(super) struct Cte {
    pub(super) name: String,
    pub(super) columns: Option<Vec<Column>>,
}

/// One query block's names
#[derive(Debug, Default)]
pub(super) struct Frame {
    /// Relations in FROM (alias or name -> relation), in order
    pub(super) relations: IndexMap<String, Relation>,
    /// CTEs defined by this query's WITH clause
    pub(super) ctes: Vec<Cte>,
    /// Columns merged by `JOIN ... USING` / `NATURAL JOIN` (lowercase): unqualified
    /// references to them are not ambiguous
    pub(super) using_columns: HashSet<String>,
    /// Output column names usable here (in ORDER BY and GROUP BY)
    pub(super) select_aliases: Vec<String>,
    /// The enclosing frame is not visible from this one (a non-LATERAL subquery in FROM
    /// can't see the FROM clause it appears in)
    pub(super) hides_parent: bool,
    /// Relations only reachable through a qualifier, never by an unqualified column
    /// or `*` (PostgreSQL 18 `RETURNING old.*, new.*`)
    pub(super) row_aliases: IndexMap<String, Relation>,
}

impl Frame {
    /// The relation of this query block that `[schema.]name` refers to.
    ///
    /// Without a schema, the relation with that alias or name. With one, the
    /// unaliased catalog table or view of that schema and name (`FROM auth.users JOIN
    /// public.users` holds two `users`); failing that, the relation registered
    /// under `name` unless it is a catalog table of another schema (with
    /// [`Catalog::strict_schemas`]: elsewhere the qualifier may name a database
    /// the schema input leaves unnamed).
    pub(super) fn qualified_relation(
        &self,
        catalog: &Catalog,
        schema: Option<&Ident>,
        name: &Ident,
    ) -> Option<&Relation> {
        let Some(schema) = schema else {
            return lookup_ignore_case(&self.relations, &name.value)
                .or_else(|| lookup_ignore_case(&self.row_aliases, &name.value));
        };
        let (schema, table) = (catalog.ident_name(schema), catalog.ident_name(name));
        let by_qualified_name = self.relations.values().find(|r| {
            r.qualified.as_ref().is_some_and(|q| {
                q.schema
                    .as_deref()
                    .is_some_and(|s| catalog.names_match(s, &schema))
                    && catalog.names_match(&q.name, &table)
            })
        });
        by_qualified_name.or_else(|| {
            lookup_ignore_case(&self.relations, &name.value)
                .filter(|r| r.qualified.is_none() || !catalog.strict_schemas)
        })
    }
}

/// The `[[database.]schema.]table` qualifier and the column of a column reference
/// (`t.c`, `s.t.c`, `db.s.t.c`); `None` for other compound identifiers
pub(super) fn split_column_ref(parts: &[Ident]) -> Option<(Option<&Ident>, &Ident, &Ident)> {
    match parts {
        [table, column] => Some((None, table, column)),
        [schema, table, column] | [_, schema, table, column] => Some((Some(schema), table, column)),
        _ => None,
    }
}

/// Result of looking up an unqualified column
pub(super) enum ColumnLookup<'s> {
    /// Found in exactly one relation (or merged by USING)
    Found(&'s Column),
    /// Found in several relations of the same query block
    Ambiguous(Vec<String>),
    /// The relation (by its key in FROM) has several columns with this name
    Duplicate(String),
    /// Not found, but a relation with unknown columns may have it
    Unknown,
    NotFound,
}

/// Stack of query blocks, innermost last
#[derive(Debug, Default)]
pub(super) struct Scope {
    frames: Vec<Frame>,
}

impl Scope {
    /// Enter a query block
    pub(super) fn push(&mut self, hides_parent: bool) {
        self.frames.push(Frame {
            hides_parent,
            ..Frame::default()
        });
    }

    /// Leave the innermost query block
    pub(super) fn pop(&mut self) -> Frame {
        self.frames.pop().unwrap_or_default()
    }

    /// The innermost query block
    pub(super) fn current(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            self.frames.push(Frame::default());
        }
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    /// The innermost query block, if any
    pub(super) fn current_ref(&self) -> Option<&Frame> {
        self.frames.last()
    }

    /// Frames visible from the innermost one, innermost first
    pub(super) fn visible(&self) -> impl Iterator<Item = &Frame> {
        let mut skip_next = false;
        self.frames.iter().rev().filter(move |frame| {
            let visible = !std::mem::take(&mut skip_next);
            if visible && frame.hides_parent {
                skip_next = true;
            }
            visible
        })
    }

    /// A relation by alias or name, innermost first
    pub(super) fn relation(&self, name: &str) -> Option<&Relation> {
        self.visible().find_map(|frame| {
            lookup_ignore_case(&frame.relations, name)
                .or_else(|| lookup_ignore_case(&frame.row_aliases, name))
        })
    }

    /// A relation by `[schema.]name` as a column reference or `schema.table.*`
    /// qualifies it, innermost first (see [`Frame::qualified_relation`])
    pub(super) fn qualified_relation(
        &self,
        catalog: &Catalog,
        schema: Option<&Ident>,
        name: &Ident,
    ) -> Option<&Relation> {
        self.visible()
            .find_map(|frame| frame.qualified_relation(catalog, schema, name))
    }

    /// A CTE by name, following the catalog's identifier case rules
    pub(super) fn cte(&self, catalog: &Catalog, name: &str) -> Option<&Cte> {
        // CTEs of hidden frames stay visible: only FROM relations are hidden
        self.frames.iter().rev().find_map(|frame| {
            frame
                .ctes
                .iter()
                .rev()
                .find(|cte| cte.name == name)
                .or_else(|| {
                    frame
                        .ctes
                        .iter()
                        .rev()
                        .find(|cte| catalog.names_match(&cte.name, name))
                })
        })
    }

    /// Set the columns of the innermost CTE named `name`
    pub(super) fn update_cte(&mut self, name: &str, columns: Option<Vec<Column>>) {
        if let Some(cte) = self
            .frames
            .iter_mut()
            .rev()
            .find_map(|frame| frame.ctes.iter_mut().rev().find(|cte| cte.name == name))
        {
            cte.columns = columns;
        }
    }

    /// Names of all visible CTEs (for suggestions)
    pub(super) fn cte_names(&self) -> Vec<String> {
        self.frames
            .iter()
            .flat_map(|f| f.ctes.iter().map(|c| c.name.clone()))
            .collect()
    }

    /// The relation an unqualified column resolves to, if it resolves to exactly one
    pub(super) fn column_relation(&self, name: &str, dialect: SqlDialect) -> Option<&Relation> {
        for frame in self.visible() {
            let mut found = frame.relations.values().filter(|r| {
                matches!(
                    r.column(name, dialect),
                    ColumnMatch::Yes(_) | ColumnMatch::Duplicate
                )
            });
            if let Some(relation) = found.next() {
                let duplicate = matches!(relation.column(name, dialect), ColumnMatch::Duplicate);
                let unique = (found.next().is_none() && !duplicate)
                    || frame.using_columns.contains(&name.to_lowercase());
                return unique.then_some(relation);
            }
            if frame
                .relations
                .values()
                .any(|r| matches!(r.column(name, dialect), ColumnMatch::Maybe))
            {
                return None;
            }
        }
        None
    }

    /// Look up an unqualified column: the innermost query block that has it wins
    pub(super) fn column(&self, name: &str, dialect: SqlDialect) -> ColumnLookup<'_> {
        for frame in self.visible() {
            let mut found: Vec<(&str, &Column)> = Vec::new();
            let mut maybe = false;
            let mut duplicate = None;
            for (key, relation) in &frame.relations {
                match relation.column(name, dialect) {
                    ColumnMatch::Yes(ty) => found.push((key, ty)),
                    ColumnMatch::Duplicate => {
                        duplicate.get_or_insert(key.as_str());
                        found.push((key, &UNKNOWN_COLUMN));
                    }
                    ColumnMatch::Maybe => maybe = true,
                    ColumnMatch::No => {}
                }
            }
            if let Some(key) = duplicate {
                // `JOIN ... USING (c)` with a side that has `c` twice is an error of
                // its own; stay silent then
                if frame.using_columns.contains(&name.to_lowercase()) {
                    return ColumnLookup::Unknown;
                }
                if found.len() == 1 {
                    return ColumnLookup::Duplicate(key.to_string());
                }
            }
            match found.len() {
                0 if maybe => return ColumnLookup::Unknown,
                0 => {}
                1 => return ColumnLookup::Found(found[0].1),
                _ if frame.using_columns.contains(&name.to_lowercase()) => {
                    return ColumnLookup::Found(found[0].1)
                }
                _ => {
                    return ColumnLookup::Ambiguous(
                        found.into_iter().map(|(k, _)| k.to_string()).collect(),
                    )
                }
            }
        }
        ColumnLookup::NotFound
    }
}

/// Look up a relation by alias or name, falling back to a case-insensitive match
pub(super) fn lookup_ignore_case<'m, V>(map: &'m IndexMap<String, V>, key: &str) -> Option<&'m V> {
    map.get(key).or_else(|| {
        map.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    })
}

/// Implicit system columns every table has
pub(super) fn is_system_column(dialect: SqlDialect, column: &str) -> bool {
    let column = column.to_ascii_lowercase();
    match dialect {
        SqlDialect::PostgreSQL => matches!(
            column.as_str(),
            "ctid" | "xmin" | "xmax" | "cmin" | "cmax" | "tableoid"
        ),
        SqlDialect::Redshift => matches!(
            column.as_str(),
            "oid" | "ctid" | "xmin" | "xmax" | "cmin" | "cmax" | "tableoid"
        ),
        SqlDialect::SQLite => matches!(column.as_str(), "rowid" | "oid" | "_rowid_"),
        // BigQuery ingestion-time partitioned tables
        SqlDialect::BigQuery => matches!(
            column.as_str(),
            "_partitiontime" | "_partitiondate" | "_table_suffix" | "_file_name"
        ),
        // Databricks file metadata and Delta change data feed columns
        SqlDialect::Databricks => matches!(
            column.as_str(),
            "_metadata" | "_change_type" | "_commit_version" | "_commit_timestamp"
        ),
        // Snowflake stage metadata columns (`metadata$filename`, ...)
        SqlDialect::Snowflake => column.starts_with("metadata$"),
        SqlDialect::MySQL => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relation(columns: &[&str]) -> Relation {
        Relation::new(
            RelationKind::Subquery,
            "t",
            Some(
                columns
                    .iter()
                    .map(|c| Column::new(*c, ExpressionType::Known(SqlType::Integer)))
                    .collect(),
            ),
        )
    }

    #[test]
    fn hidden_frame_is_skipped_but_outer_frames_stay_visible() {
        let mut scope = Scope::default();
        scope.push(false);
        scope
            .current()
            .relations
            .insert("outer_q".into(), relation(&["a"]));
        scope.push(false);
        scope
            .current()
            .relations
            .insert("from_item".into(), relation(&["b"]));
        // Non-LATERAL derived table in FROM: hides the FROM clause it appears in
        scope.push(true);
        assert!(scope.relation("from_item").is_none());
        assert!(scope.relation("outer_q").is_some());
        assert!(matches!(
            scope.column("b", SqlDialect::PostgreSQL),
            ColumnLookup::NotFound
        ));
        assert!(matches!(
            scope.column("a", SqlDialect::PostgreSQL),
            ColumnLookup::Found(_)
        ));
    }

    #[test]
    fn innermost_block_wins_over_outer_ambiguity() {
        let mut scope = Scope::default();
        scope.push(false);
        scope
            .current()
            .relations
            .insert("x".into(), relation(&["id"]));
        scope
            .current()
            .relations
            .insert("y".into(), relation(&["id"]));
        scope.push(false);
        scope
            .current()
            .relations
            .insert("z".into(), relation(&["id"]));
        assert!(matches!(
            scope.column("id", SqlDialect::PostgreSQL),
            ColumnLookup::Found(_)
        ));
        scope.pop();
        assert!(matches!(
            scope.column("id", SqlDialect::PostgreSQL),
            ColumnLookup::Ambiguous(_)
        ));
    }
}
