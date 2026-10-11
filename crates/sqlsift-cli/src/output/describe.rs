//! Output of `sqlsift describe`: the output columns and types of each query.
//!
//! The JSON shape is meant for tools (type generators, editors): a name or type
//! sqlsift can't tell is `null`; `nullable` is `false` only when the column
//! certainly can't be NULL.

use serde_json::{json, Value};
use sqlsift_core::{QueryDescription, SqlDialect};

use crate::args::DescribeFormat;

/// The described queries of one file
pub struct FileDescription {
    pub file: String,
    pub queries: Vec<QueryDescription>,
}

/// Render the descriptions of `files` in the given format
pub fn render(files: &[FileDescription], dialect: SqlDialect, format: DescribeFormat) -> String {
    match format {
        DescribeFormat::Human => to_human(files, dialect),
        DescribeFormat::Json => {
            let mut out = serde_json::to_string_pretty(&to_json(files, dialect))
                .expect("describe JSON is serializable");
            out.push('\n');
            out
        }
    }
}

fn to_json(files: &[FileDescription], dialect: SqlDialect) -> Value {
    let queries: Vec<Value> = files
        .iter()
        .flat_map(|f| {
            f.queries.iter().map(|q| {
                let columns = q.columns.as_ref().map(|columns| {
                    columns
                        .iter()
                        .map(|c| {
                            json!({
                                "name": c.name,
                                "type": c.ty.as_ref().map(|t| t.display_name_in(dialect)),
                                "nullable": c.nullable,
                            })
                        })
                        .collect::<Vec<_>>()
                });
                json!({
                    "file": f.file,
                    "line": q.line,
                    "column": q.column,
                    "name": q.name,
                    "kind": q.kind.as_str(),
                    "columns": columns,
                    "parameters": q.parameters.iter().map(|p| json!({
                        "index": p.index,
                        "name": p.name,
                        "type": p.ty.as_ref().map(|t| t.display_name_in(dialect)),
                    })).collect::<Vec<_>>(),
                })
            })
        })
        .collect();
    Value::Array(queries)
}

fn to_human(files: &[FileDescription], dialect: SqlDialect) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    for f in files {
        for q in &f.queries {
            let _ = write!(out, "{}:{}", f.file, q.line);
            if let Some(name) = &q.name {
                let _ = write!(out, " {name}");
            }
            let _ = writeln!(out, " ({})", q.kind.as_str());
            if !q.parameters.is_empty() {
                let parameters: Vec<String> = q
                    .parameters
                    .iter()
                    .map(|p| {
                        let ty =
                            p.ty.as_ref()
                                .map_or_else(|| "?".to_string(), |t| t.display_name_in(dialect));
                        match &p.name {
                            Some(name) => format!("{name} {ty}"),
                            None => format!("${} {ty}", p.index),
                        }
                    })
                    .collect();
                let _ = writeln!(out, "  parameters: {}", parameters.join(", "));
            }
            match &q.columns {
                None => {
                    let _ = writeln!(out, "  (columns unknown)");
                }
                Some(columns) if columns.is_empty() => {
                    let _ = writeln!(out, "  (no columns)");
                }
                Some(columns) => {
                    let names: Vec<&str> = columns
                        .iter()
                        .map(|c| c.name.as_deref().unwrap_or("?"))
                        .collect();
                    let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
                    for (name, column) in names.iter().zip(columns) {
                        let ty = column
                            .ty
                            .as_ref()
                            .map_or_else(|| "?".to_string(), |t| t.display_name_in(dialect));
                        let null = match column.nullable {
                            Some(false) => " not null",
                            Some(true) => " null",
                            None => "",
                        };
                        let _ = writeln!(out, "  {name:<width$}  {ty}{null}");
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlsift_core::schema::SchemaBuilder;
    use sqlsift_core::Analyzer;

    fn describe(sql: &str) -> Vec<FileDescription> {
        let mut builder = SchemaBuilder::new();
        builder
            .parse("CREATE TABLE users (id BIGINT PRIMARY KEY, email TEXT);")
            .unwrap();
        let (catalog, _) = builder.build();
        let queries = Analyzer::new(&catalog).describe(sql).queries;
        vec![FileDescription {
            file: "q.sql".to_string(),
            queries,
        }]
    }

    #[test]
    fn json_shape() {
        let files =
            describe("-- name: GetUser :one\nSELECT id, email, 1 FROM users WHERE id = $1;");
        let json = to_json(&files, SqlDialect::PostgreSQL);
        assert_eq!(
            json,
            json!([{
                "file": "q.sql",
                "line": 2,
                "column": 1,
                "name": "GetUser",
                "kind": "select",
                "columns": [
                    { "name": "id", "type": "bigint", "nullable": false },
                    { "name": "email", "type": "text", "nullable": true },
                    { "name": null, "type": "integer", "nullable": false },
                ],
                "parameters": [{ "index": 1, "name": null, "type": "bigint" }],
            }])
        );
    }

    #[test]
    fn unknown_columns_are_null() {
        let files = describe("SELECT * FROM missing");
        let json = to_json(&files, SqlDialect::PostgreSQL);
        assert_eq!(json[0]["columns"], Value::Null);
    }

    #[test]
    fn human_listing() {
        let files = describe("SELECT id, email AS e FROM users; DELETE FROM users;");
        assert_eq!(
            to_human(&files, SqlDialect::PostgreSQL),
            "q.sql:1 (select)\n  id  bigint not null\n  e   text null\nq.sql:1 (delete)\n  (no columns)\n"
        );
    }
}
