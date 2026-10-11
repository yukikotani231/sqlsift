//! dbt catalog support (alpha)
//!
//! `dbt docs generate` writes `target/catalog.json`: the columns (and warehouse
//! types) of every model, seed, snapshot and source relation of a dbt project.
//! [`load_catalog_json`] adds them to a [`Catalog`], so that with
//! [`Templating::Jinja`](crate::Templating::Jinja) the columns of
//! `{{ ref('orders') }}` and `{{ source('shop', 'orders') }}` are known:
//!
//! - A model, seed or snapshot becomes the table `ref.<name>` (in the
//!   [`REF_SCHEMA`] schema), which `{{ ref('<name>') }}` is masked to.
//! - A source table becomes `<source name>.<table name>`, which
//!   `{{ source('<source name>', '<table name>') }}` is masked to.
//! - Every relation is also added under its warehouse name (`<schema>.<name>`),
//!   for SQL that names it directly.
//!
//! The catalog keys (dbt unique ids such as `model.jaffle.orders` and
//! `source.jaffle.shop.orders`) hold the names that `ref()` and `source()` take, so
//! `manifest.json` isn't needed. Names that can't be told apart (a model of the
//! same name in two packages, versioned models) are left out, and a `ref()` or
//! `source()` of a relation that isn't in the catalog stays an unknown table: a
//! stale or partial catalog never makes a column unknown to sqlsift reported.
//!
//! Tables already in the catalog (from the schema files) are kept as they are.
//! Column types are warehouse type names: the common ones are mapped to
//! [`SqlType`]s (semi-structured ones such as `VARIANT` to
//! [`SqlType::SemiStructured`]) and any other to [`SqlType::Unknown`]: values of
//! these two are never reported on.

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

use crate::schema::{Catalog, ColumnDef, QualifiedName, TableDef};
use crate::types::SqlType;

/// The schema that the relations of dbt models, seeds and snapshots are added to,
/// by model name: `{{ ref('orders') }}` is masked to `ref.orders`
pub const REF_SCHEMA: &str = "ref";

/// The schema that relation variables are added to, by variable name:
/// `{{ var('orders') }}` is masked to `var.orders`
pub const VAR_SCHEMA: &str = "var";

/// Variables of a dbt project whose value is a relation, from the `vars:` of its
/// `dbt_project.yml` (fivetran packages: `netsuite_vendors: "{{ source('netsuite',
/// 'vendors') }}"`). [`RelationVars::add_to_catalog`] adds each one whose relation
/// the catalog has as `var.<name>`, so `FROM {{ var('netsuite_vendors') }}` is that
/// relation. Variable name → the `ref(...)` / `source(...)` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelationVars(HashMap<String, String>);

impl RelationVars {
    /// The relation variables of a `dbt_project.yml`. Only values that are a single
    /// `{{ ref(...) }}` or `{{ source(...) }}` with literal arguments are kept;
    /// variables nested under a package name are read like top-level ones.
    pub fn from_dbt_project(yml: &str) -> Self {
        let mut vars = HashMap::new();
        let mut in_vars = false;
        for line in yml.lines() {
            let code = line.trim_end();
            if code.trim_start().is_empty() || code.trim_start().starts_with('#') {
                continue;
            }
            if !code.starts_with([' ', '\t']) {
                in_vars = code.starts_with("vars:");
                continue;
            }
            if !in_vars {
                continue;
            }
            let Some((key, value)) = code.trim_start().split_once(':') else {
                continue;
            };
            let key = key.trim().trim_matches(['\'', '"']);
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            let Some(call) = value
                .strip_prefix("{{")
                .and_then(|v| v.strip_suffix("}}"))
                .map(str::trim)
            else {
                continue;
            };
            if crate::templating::relation_call(call).is_some() {
                vars.insert(key.to_string(), call.to_string());
            }
        }
        RelationVars(vars)
    }

    /// The `ref(...)` / `source(...)` call variable `name` holds
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Whether there are no relation variables
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Add the variables of `other` (a variable both define differently is left out:
    /// it can't be told which one a model uses)
    pub fn merge(&mut self, other: RelationVars) {
        for (name, call) in other.0 {
            match self.0.get(&name) {
                Some(existing) if *existing != call => {
                    self.0.insert(name, String::new());
                }
                _ => {
                    self.0.insert(name, call);
                }
            }
        }
    }

    /// Add each variable whose relation `catalog` has (a table: from the schema
    /// files or a dbt catalog) as the table `var.<name>`. Returns how many were
    /// added.
    pub fn add_to_catalog(&self, catalog: &mut Catalog) -> usize {
        let mut added = 0;
        for (name, call) in &self.0 {
            let Some((schema, table)) = crate::templating::relation_call(call) else {
                continue;
            };
            let Some(columns) = catalog
                .get_table(&QualifiedName::with_schema(schema, table))
                .map(|t| t.columns.clone())
            else {
                continue;
            };
            if add(
                catalog,
                QualifiedName::with_schema(VAR_SCHEMA, name),
                &columns,
            ) {
                added += 1;
            }
        }
        added
    }
}

/// What [`load_catalog_json`] added to the catalog
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbtCatalogSummary {
    /// Models, seeds and snapshots that `ref()` resolves to
    pub refs: usize,
    /// Source tables that `source()` resolves to
    pub sources: usize,
}

/// A relation of the dbt catalog
struct Relation {
    /// The warehouse schema and name
    schema: Option<String>,
    name: Option<String>,
    columns: IndexMap<String, ColumnDef>,
}

/// Add the relations of a dbt `catalog.json` to `catalog` (see the module docs).
///
/// # Errors
///
/// Returns a message when `json` isn't a dbt catalog.
pub fn load_catalog_json(json: &str, catalog: &mut Catalog) -> Result<DbtCatalogSummary, String> {
    let root: Value =
        serde_json::from_str(json).map_err(|e| format!("invalid dbt catalog JSON: {e}"))?;
    let nodes = root.get("nodes").and_then(Value::as_object);
    let sources = root.get("sources").and_then(Value::as_object);
    if nodes.is_none() && sources.is_none() {
        return Err("not a dbt catalog (no \"nodes\" or \"sources\")".to_string());
    }

    // ref name -> relations of that name (more than one can't be told apart)
    let mut refs: IndexMap<String, Vec<Relation>> = IndexMap::new();
    let mut source_tables: HashMap<(String, String), Vec<Relation>> = HashMap::new();
    let mut source_order: Vec<(String, String)> = Vec::new();
    let mut warehouse: Vec<Relation> = Vec::new();

    for (unique_id, node) in nodes.into_iter().flatten() {
        let parts: Vec<&str> = unique_id.split('.').collect();
        let Some(relation) = relation(node) else {
            continue;
        };
        // `model.<package>.<name>`; versioned models (`model.<package>.<name>.v2`)
        // are left out
        match parts.as_slice() {
            ["model" | "seed" | "snapshot", _package, name] => {
                refs.entry((*name).to_string()).or_default().push(relation);
            }
            _ => warehouse.push(relation),
        }
    }
    for (unique_id, node) in sources.into_iter().flatten() {
        let parts: Vec<&str> = unique_id.split('.').collect();
        let Some(relation) = relation(node) else {
            continue;
        };
        // `source.<package>.<source name>.<table name>`
        if let ["source", _package, source, table] = parts.as_slice() {
            let key = ((*source).to_string(), (*table).to_string());
            if !source_tables.contains_key(&key) {
                source_order.push(key.clone());
            }
            source_tables.entry(key).or_default().push(relation);
        } else {
            warehouse.push(relation);
        }
    }

    let mut summary = DbtCatalogSummary::default();
    for (name, relations) in &refs {
        if let [relation] = relations.as_slice() {
            let qualified = QualifiedName::with_schema(REF_SCHEMA, name.as_str());
            if add(catalog, qualified, &relation.columns) {
                summary.refs += 1;
            }
        }
    }
    for key in &source_order {
        if let [relation] = source_tables[key].as_slice() {
            let qualified = QualifiedName::with_schema(key.0.as_str(), key.1.as_str());
            if add(catalog, qualified, &relation.columns) {
                summary.sources += 1;
            }
        }
    }
    // Warehouse names: of every relation, unless two have the same name
    let all = refs
        .values()
        .flatten()
        .chain(source_order.iter().flat_map(|key| &source_tables[key]))
        .chain(&warehouse);
    let mut by_name: IndexMap<(String, String), Vec<&Relation>> = IndexMap::new();
    for relation in all {
        if let (Some(schema), Some(name)) = (&relation.schema, &relation.name) {
            by_name
                .entry((schema.clone(), name.clone()))
                .or_default()
                .push(relation);
        }
    }
    for ((schema, name), relations) in by_name {
        if let [relation] = relations.as_slice() {
            add(
                catalog,
                QualifiedName::with_schema(schema, name),
                &relation.columns,
            );
        }
    }
    Ok(summary)
}

/// Add a table named `name` with `columns`, unless the catalog has a table or view
/// of that name
fn add(catalog: &mut Catalog, name: QualifiedName, columns: &IndexMap<String, ColumnDef>) -> bool {
    if catalog.table_exists(&name) || catalog.view_exists(&name) {
        return false;
    }
    let mut table = TableDef::new(name);
    table.columns.clone_from(columns);
    catalog.add_table(table);
    true
}

/// The relation of a catalog node or source: its warehouse name and columns, in
/// column order. `None` when it has no columns object.
fn relation(node: &Value) -> Option<Relation> {
    let metadata = node.get("metadata");
    let text = |key: &str| {
        metadata
            .and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let mut columns: Vec<(i64, String, SqlType)> = node
        .get("columns")?
        .as_object()?
        .iter()
        .map(|(key, column)| {
            let name = column
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_string();
            let index = column.get("index").and_then(Value::as_i64).unwrap_or(0);
            let data_type = column
                .get("type")
                .and_then(Value::as_str)
                .map_or(SqlType::Unknown, warehouse_type);
            (index, name, data_type)
        })
        .collect();
    columns.sort_by_key(|(index, _, _)| *index);
    Some(Relation {
        schema: text("schema"),
        name: text("name"),
        columns: columns
            .into_iter()
            .map(|(_, name, data_type)| (name.clone(), ColumnDef::new(name, data_type)))
            .collect(),
    })
}

/// The [`SqlType`] of a warehouse type name from `catalog.json` (PostgreSQL,
/// Snowflake, BigQuery, Redshift, Databricks, DuckDB names). Any type not known
/// for sure is [`SqlType::Unknown`].
pub fn warehouse_type(name: &str) -> SqlType {
    let lower = name.trim().to_ascii_lowercase();
    // The type name without its parameters (`varchar(256)`, `number(38,0)`)
    let base = lower.split('(').next().unwrap_or("").trim();
    // The type's numeric parameters (`number(38,0)` -> 38, 0)
    let params: Vec<Option<u64>> = lower
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(inner, _)| inner.split(',').map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_default();
    let param = |i: usize| params.get(i).copied().flatten();
    match base {
        "smallint" | "int2" => SqlType::SmallInt,
        "integer" | "int" | "int4" => SqlType::Integer,
        "bigint" | "int8" | "int64" | "long" => SqlType::BigInt,
        "numeric" | "decimal" | "number" | "bignumeric" | "dec" => SqlType::Decimal {
            precision: param(0),
            scale: param(1),
        },
        "byteint" => SqlType::TinyInt,
        "real" | "float4" => SqlType::Real,
        "double precision" | "double" | "float" | "float8" | "float64" => SqlType::DoublePrecision,
        "text" | "string" | "varchar" | "character varying" | "char" | "character" | "nvarchar"
        | "bpchar" => SqlType::Text,
        "boolean" | "bool" => SqlType::Boolean,
        "date" => SqlType::Date,
        "timestamp with time zone" | "timestamptz" | "timestamp_tz" | "timestamp_ltz" => {
            SqlType::Timestamp {
                precision: None,
                with_timezone: true,
            }
        }
        "timestamp" | "timestamp without time zone" | "timestamp_ntz" | "datetime" => {
            SqlType::Timestamp {
                precision: None,
                with_timezone: false,
            }
        }
        "time" | "time without time zone" => SqlType::Time {
            precision: None,
            with_timezone: false,
        },
        "uuid" => SqlType::Uuid,
        "json" => SqlType::Json,
        "jsonb" => SqlType::Jsonb,
        "bytea" | "binary" | "bytes" | "varbinary" => SqlType::Bytea,
        // Snowflake semi-structured and spatial types (never type checked, like
        // `SqlType::from_ast` maps them)
        "variant" | "object" | "geography" | "geometry" => SqlType::SemiStructured(base.into()),
        "array" => SqlType::SemiStructured("array".into()),
        _ => SqlType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{
        "metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/catalog/v1.json"},
        "nodes": {
            "model.jaffle.orders": {
                "metadata": {"type": "BASE TABLE", "schema": "analytics", "name": "orders", "database": "db"},
                "columns": {
                    "status": {"type": "text", "index": 2, "name": "status"},
                    "order_id": {"type": "integer", "index": 1, "name": "order_id"},
                    "payload": {"type": "VARIANT", "index": 3, "name": "payload"}
                },
                "unique_id": "model.jaffle.orders"
            },
            "model.jaffle.dup": {"metadata": {"schema": "a", "name": "dup"}, "columns": {}},
            "model.other.dup": {"metadata": {"schema": "b", "name": "dup"}, "columns": {}},
            "model.jaffle.versioned.v2": {"metadata": {"schema": "a", "name": "versioned_v2"}, "columns": {}}
        },
        "sources": {
            "source.jaffle.shop.raw_orders": {
                "metadata": {"schema": "raw", "name": "orders"},
                "columns": {"ID": {"type": "NUMBER(38,0)", "index": 1, "name": "ID"}}
            }
        }
    }"#;

    #[test]
    fn relation_vars_of_dbt_project() {
        let yml = "name: 'shop'\nmodels:\n  shop:\n    orders: \"{{ ref('orders') }}\"\n\
                   vars:\n  shop:\n    # comment\n    orders_table: \"{{ ref('orders') }}\"\n\
                   \x20   raw_orders: '{{ source(\"shop\", \"raw_orders\") }}'\n\
                   \x20   missing: \"{{ ref('missing') }}\"\n    flag: true\n\
                   \x20   computed: \"{{ ref(var('x')) }}\"\n  top_level: \"{{ ref('orders') }}\"\n\
                   seeds:\n  a: \"{{ ref('orders') }}\"\n";
        let vars = RelationVars::from_dbt_project(yml);
        assert_eq!(vars.get("orders_table"), Some("ref('orders')"));
        assert_eq!(
            vars.get("raw_orders"),
            Some("source(\"shop\", \"raw_orders\")")
        );
        assert_eq!(vars.get("top_level"), Some("ref('orders')"));
        assert_eq!(vars.get("flag"), None);
        assert_eq!(vars.get("computed"), None);
        // Only `vars:`
        assert_eq!(vars.get("a"), None);

        let mut catalog = Catalog::new();
        load_catalog_json(CATALOG, &mut catalog).unwrap();
        assert_eq!(vars.add_to_catalog(&mut catalog), 3);
        let table = catalog
            .get_table(&QualifiedName::with_schema(VAR_SCHEMA, "orders_table"))
            .unwrap();
        assert_eq!(table.column_names(), ["order_id", "status", "payload"]);
        assert!(catalog.table_exists(&QualifiedName::with_schema(VAR_SCHEMA, "raw_orders")));
        assert!(!catalog.table_exists(&QualifiedName::with_schema(VAR_SCHEMA, "missing")));

        // A variable two projects define differently is left out
        let mut merged = vars.clone();
        merged.merge(RelationVars::from_dbt_project(
            "vars:\n  orders_table: \"{{ ref('other') }}\"\n  top_level: \"{{ ref('orders') }}\"\n",
        ));
        let mut catalog = Catalog::new();
        load_catalog_json(CATALOG, &mut catalog).unwrap();
        merged.add_to_catalog(&mut catalog);
        assert!(!catalog.table_exists(&QualifiedName::with_schema(VAR_SCHEMA, "orders_table")));
        assert!(catalog.table_exists(&QualifiedName::with_schema(VAR_SCHEMA, "top_level")));
    }

    #[test]
    fn loads_refs_sources_and_warehouse_names() {
        let mut catalog = Catalog::new();
        let summary = load_catalog_json(CATALOG, &mut catalog).unwrap();
        assert_eq!(
            summary,
            DbtCatalogSummary {
                refs: 1,
                sources: 1
            }
        );

        let orders = catalog
            .get_table(&QualifiedName::with_schema("ref", "orders"))
            .unwrap();
        assert_eq!(orders.column_names(), ["order_id", "status", "payload"]);
        assert_eq!(orders.columns["order_id"].data_type, SqlType::Integer);
        assert_eq!(
            orders.columns["payload"].data_type,
            SqlType::SemiStructured("variant".into())
        );
        assert!(catalog.table_exists(&QualifiedName::with_schema("analytics", "orders")));

        let source = catalog
            .get_table(&QualifiedName::with_schema("shop", "raw_orders"))
            .unwrap();
        assert_eq!(
            source.columns["ID"].data_type,
            SqlType::Decimal {
                precision: Some(38),
                scale: Some(0)
            }
        );
        assert!(catalog.table_exists(&QualifiedName::with_schema("raw", "orders")));

        // Ambiguous and versioned models are left out of `ref`
        assert!(!catalog.table_exists(&QualifiedName::with_schema("ref", "dup")));
        assert!(!catalog.table_exists(&QualifiedName::with_schema("ref", "versioned")));
        assert!(catalog.table_exists(&QualifiedName::with_schema("a", "versioned_v2")));
    }

    #[test]
    fn keeps_tables_of_the_schema_files() {
        let mut catalog = Catalog::new();
        let mut table = TableDef::new(QualifiedName::with_schema("raw", "orders"));
        table
            .columns
            .insert("x".into(), ColumnDef::new("x", SqlType::Text));
        catalog.add_table(table);
        load_catalog_json(CATALOG, &mut catalog).unwrap();
        let orders = catalog
            .get_table(&QualifiedName::with_schema("raw", "orders"))
            .unwrap();
        assert_eq!(orders.column_names(), ["x"]);
    }

    #[test]
    fn rejects_other_json() {
        let mut catalog = Catalog::new();
        assert!(load_catalog_json("{", &mut catalog).is_err());
        assert!(load_catalog_json("{\"a\": 1}", &mut catalog).is_err());
    }

    #[test]
    fn maps_warehouse_types() {
        assert_eq!(warehouse_type("INT64"), SqlType::BigInt);
        assert_eq!(warehouse_type("character varying(256)"), SqlType::Text);
        assert_eq!(warehouse_type("STRING"), SqlType::Text);
        assert_eq!(warehouse_type("TIMESTAMP_NTZ"), warehouse_type("timestamp"));
        assert_eq!(warehouse_type("ARRAY<STRING>"), SqlType::Unknown);
        assert_eq!(
            warehouse_type("geography"),
            SqlType::SemiStructured("geography".into())
        );
        assert_eq!(
            warehouse_type("VARIANT"),
            SqlType::SemiStructured("variant".into())
        );
        assert_eq!(
            warehouse_type("ARRAY"),
            SqlType::SemiStructured("array".into())
        );
        assert_eq!(
            warehouse_type("NUMBER(12, 2)"),
            SqlType::Decimal {
                precision: Some(12),
                scale: Some(2)
            }
        );
        assert_eq!(
            warehouse_type("TIMESTAMP_LTZ").display_name(),
            "timestamp with time zone"
        );
    }
}
