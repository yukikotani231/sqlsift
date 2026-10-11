//! Reading the schema, queries and engine of sqlc configuration files
//! (`sqlc.yaml`, `sqlc.yml`, `sqlc.json`), version 1 (`packages:`) and 2 (`sql:`)
//!
//! YAML is read with a small line-based reader that understands what sqlc
//! configurations use for these keys: `key: value`, `key: [a, b]` and block lists.

/// One `sql:` (version 2) or `packages:` (version 1) entry of a sqlc configuration
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SqlcEntry {
    pub engine: Option<String>,
    /// Schema files or directories, relative to the configuration file
    pub schema: Vec<String>,
    /// Query files or directories, relative to the configuration file
    pub queries: Vec<String>,
}

/// The entries of a sqlc configuration file, parsed as JSON when `json`, else YAML
pub fn parse(text: &str, json: bool) -> Vec<SqlcEntry> {
    if json {
        parse_json(text)
    } else {
        parse_yaml(text)
    }
}

fn parse_json(text: &str) -> Vec<SqlcEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let strings = |v: Option<&serde_json::Value>| -> Vec<String> {
        match v {
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|i| i.as_str().map(ToString::to_string))
                .collect(),
            _ => Vec::new(),
        }
    };
    let entries = value
        .get("sql")
        .or_else(|| value.get("packages"))
        .and_then(serde_json::Value::as_array);
    entries
        .into_iter()
        .flatten()
        .map(|entry| SqlcEntry {
            engine: entry
                .get("engine")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            schema: strings(entry.get("schema")),
            queries: strings(entry.get("queries")),
        })
        .filter(|e| !e.schema.is_empty() || !e.queries.is_empty())
        .collect()
}

/// Keys of an entry that are read
const KEYS: &[&str] = &["engine", "schema", "queries"];

fn parse_yaml(text: &str) -> Vec<SqlcEntry> {
    let mut entries: Vec<SqlcEntry> = Vec::new();
    let mut current = SqlcEntry::default();
    let mut seen: Vec<&str> = Vec::new();
    // Column of the entry keys, taken from the first one found
    let mut key_column: Option<usize> = None;
    // The list key whose block items (`- value`) follow, and its column
    let mut open_list: Option<(&str, usize)> = None;

    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();

        // A block list item of the open key
        if let Some((key, column)) = open_list {
            if indent > column || (indent == column && trimmed.starts_with('-')) {
                if let Some(item) = trimmed.strip_prefix('-') {
                    let item = unquote(item.trim());
                    if !item.is_empty() && !item.contains(": ") {
                        push(&mut current, key, item.to_string());
                        continue;
                    }
                }
            }
            open_list = None;
        }

        // `key: value`, possibly as the first key of a list item (`- key: value`)
        let (column, body) = match trimmed.strip_prefix("- ") {
            Some(rest) => (
                indent + 2 + (rest.len() - rest.trim_start().len()),
                rest.trim(),
            ),
            None => (indent, trimmed),
        };
        let Some((key, value)) = body.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let Some(&key) = KEYS.iter().find(|k| **k == key) else {
            continue;
        };
        if key_column.is_some_and(|c| c != column) {
            continue;
        }
        key_column = Some(column);
        // A key seen again starts the next entry
        if seen.contains(&key) {
            entries.push(std::mem::take(&mut current));
            seen.clear();
        }
        seen.push(key);

        let value = value.trim();
        if value.is_empty() {
            open_list = Some((key, column));
        } else if let Some(list) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            for item in list.split(',') {
                let item = unquote(item.trim());
                if !item.is_empty() {
                    push(&mut current, key, item.to_string());
                }
            }
        } else {
            push(&mut current, key, unquote(value).to_string());
        }
    }
    entries.push(current);
    entries
        .into_iter()
        .filter(|e| !e.schema.is_empty() || !e.queries.is_empty())
        .collect()
}

fn push(entry: &mut SqlcEntry, key: &str, value: String) {
    match key {
        "engine" => entry.engine = Some(value),
        "schema" => entry.schema.push(value),
        "queries" => entry.queries.push(value),
        _ => {}
    }
}

/// `line` without a trailing `# comment` (a `#` at the start or after whitespace,
/// outside quotes)
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && previous.is_whitespace() => return &line[..i],
            None => {}
        }
        previous = c;
    }
    line
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(engine: &str, schema: &[&str], queries: &[&str]) -> SqlcEntry {
        SqlcEntry {
            engine: Some(engine.to_string()),
            schema: schema.iter().map(ToString::to_string).collect(),
            queries: queries.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn version_2_yaml() {
        let yaml = r#"
version: "2"
sql:
  - engine: "postgresql"
    queries: "query.sql"   # the queries
    schema: "schema.sql"
    gen:
      go:
        package: "tutorial"
        out: "tutorial"
        overrides:
          - db_type: "uuid"
            go_type: "github.com/google/uuid.UUID"
  - schema:
      - "mysql/schema"
      - 'mysql/extra.sql'
    queries: [mysql/queries, "mysql/more.sql"]
    engine: mysql
"#;
        assert_eq!(
            parse(yaml, false),
            vec![
                entry("postgresql", &["schema.sql"], &["query.sql"]),
                entry(
                    "mysql",
                    &["mysql/schema", "mysql/extra.sql"],
                    &["mysql/queries", "mysql/more.sql"]
                ),
            ]
        );
    }

    #[test]
    fn version_1_yaml() {
        let yaml = "version: \"1\"\npackages:\n  - name: \"db\"\n    path: \"internal/db\"\n    queries: \"./sql/query/\"\n    schema: \"./sql/schema/\"\n    engine: \"postgresql\"\n";
        assert_eq!(
            parse(yaml, false),
            vec![entry("postgresql", &["./sql/schema/"], &["./sql/query/"])]
        );
    }

    #[test]
    fn json() {
        let json = r#"{"version": "2", "sql": [{"engine": "sqlite", "schema": ["a.sql", "b.sql"], "queries": "q"}]}"#;
        assert_eq!(
            parse(json, true),
            vec![entry("sqlite", &["a.sql", "b.sql"], &["q"])]
        );
        assert!(parse("not json", true).is_empty());
    }
}
