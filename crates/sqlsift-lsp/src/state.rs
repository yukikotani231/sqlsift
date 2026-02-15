use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::Url;

use sqlsift_core::schema::{Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::{Analyzer, Diagnostic, SqlDialect};

use crate::config::Config;

pub struct ServerState {
    pub catalog: Catalog,
    pub dialect: SqlDialect,
    pub disabled_rules: HashSet<String>,
    pub open_documents: HashMap<Url, String>,
    pub schema_files: Vec<PathBuf>,
    pub workspace_root: Option<PathBuf>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            catalog: Catalog::default(),
            dialect: SqlDialect::default(),
            disabled_rules: HashSet::new(),
            open_documents: HashMap::new(),
            schema_files: Vec::new(),
            workspace_root: None,
        }
    }

    /// Load configuration from sqlsift.toml and set up state
    pub fn load_config(&mut self, workspace_root: &Path) {
        self.workspace_root = Some(workspace_root.to_path_buf());

        if let Some(config) = Config::find_from_root(workspace_root) {
            // Resolve dialect
            if let Some(dialect_str) = &config.dialect {
                if let Ok(d) = dialect_str.parse() {
                    self.dialect = d;
                }
            }

            // Set disabled rules
            self.disabled_rules = config.disable.iter().cloned().collect();

            // Resolve schema files
            self.schema_files = resolve_schema_files(&config, workspace_root);
        }
    }

    /// Rebuild the catalog from schema files
    pub fn rebuild_catalog(&mut self) -> Vec<String> {
        let mut builder = SchemaBuilder::with_dialect(self.dialect);
        let mut errors = Vec::new();

        for schema_file in &self.schema_files {
            match std::fs::read_to_string(schema_file) {
                Ok(content) => {
                    if let Err(diags) = builder.parse(&content) {
                        for d in diags {
                            errors.push(format!("{}: {}", schema_file.display(), d.message));
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Failed to read {}: {}", schema_file.display(), e));
                }
            }
        }

        let (catalog, schema_diags) = builder.build();
        self.catalog = catalog;

        for d in schema_diags {
            errors.push(format!("Schema warning: {}", d.message));
        }

        errors
    }

    /// Analyze a SQL document and return diagnostics
    pub fn analyze_document(&self, text: &str) -> Vec<Diagnostic> {
        let mut analyzer = Analyzer::with_dialect(&self.catalog, self.dialect);
        analyzer.analyze(text)
    }

    /// Check if a file path is one of the schema files
    pub fn is_schema_file(&self, path: &Path) -> bool {
        self.schema_files.iter().any(|p| p == path)
    }

    /// Get hover information for a word (table, view, or column name)
    pub fn hover_info(&self, word: &str) -> Option<String> {
        let name = QualifiedName::new(word);

        // Check tables
        if let Some(table) = self.catalog.get_table(&name) {
            let mut md = format!("**{}** (table)\n\n", table.name.name);
            md.push_str("| Column | Type | Nullable |\n");
            md.push_str("|--------|------|----------|\n");
            for col in table.columns.values() {
                let nullable = if col.nullable { "NULL" } else { "NOT NULL" };
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    col.name,
                    col.data_type.display_name(),
                    nullable
                ));
            }
            return Some(md);
        }

        // Check views
        if let Some(view) = self.catalog.get_view(&name) {
            let kind = if view.materialized {
                "materialized view"
            } else {
                "view"
            };
            let cols = view.columns.join(", ");
            return Some(format!(
                "**{}** ({})\n\nColumns: {}",
                view.name.name, kind, cols
            ));
        }

        // Check columns across all tables
        let mut matches = Vec::new();
        for schema in self.catalog.schemas.values() {
            for table in schema.tables.values() {
                if let Some(col) = table.get_column(word) {
                    let nullable = if col.nullable { "nullable" } else { "not null" };
                    matches.push(format!(
                        "**{}** — {} ({})\n\nTable: {}",
                        col.name,
                        col.data_type.display_name(),
                        nullable,
                        table.name.name
                    ));
                }
            }
        }

        if matches.is_empty() {
            None
        } else {
            Some(matches.join("\n\n---\n\n"))
        }
    }
}

/// Resolve schema file paths from config (handles glob patterns and schema_dir)
fn resolve_schema_files(config: &Config, workspace_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for pattern in &config.schema {
        let abs_pattern = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            workspace_root.join(pattern).display().to_string()
        };

        match glob::glob(&abs_pattern) {
            Ok(paths) => {
                for path in paths.flatten() {
                    files.push(path);
                }
            }
            Err(_) => {
                // If glob fails, try as literal path
                let path = workspace_root.join(pattern);
                if path.exists() {
                    files.push(path);
                }
            }
        }
    }

    if let Some(dir) = &config.schema_dir {
        let abs_dir = if Path::new(dir).is_absolute() {
            dir.clone()
        } else {
            workspace_root.join(dir).display().to_string()
        };
        let pattern = format!("{abs_dir}/**/*.sql");
        if let Ok(paths) = glob::glob(&pattern) {
            for path in paths.flatten() {
                files.push(path);
            }
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_schema(schema_sql: &str) -> ServerState {
        let mut state = ServerState::new();
        let mut builder = SchemaBuilder::new();
        builder.parse(schema_sql).unwrap();
        let (catalog, _) = builder.build();
        state.catalog = catalog;
        state
    }

    #[test]
    fn test_analyze_document_valid_query() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER, name TEXT);");
        let diagnostics = state.analyze_document("SELECT id, name FROM users");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn test_analyze_document_table_not_found() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER, name TEXT);");
        let diagnostics = state.analyze_document("SELECT * FROM nonexistent");
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].code(), "E0001");
    }

    #[test]
    fn test_analyze_document_column_not_found() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER, name TEXT);");
        let diagnostics = state.analyze_document("SELECT bad_column FROM users");
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].code(), "E0002");
    }

    #[test]
    fn test_is_schema_file() {
        let mut state = ServerState::new();
        state.schema_files.push(PathBuf::from("/tmp/schema.sql"));
        assert!(state.is_schema_file(Path::new("/tmp/schema.sql")));
        assert!(!state.is_schema_file(Path::new("/tmp/other.sql")));
    }

    #[test]
    fn test_new_state_defaults() {
        let state = ServerState::new();
        assert!(state.open_documents.is_empty());
        assert!(state.schema_files.is_empty());
        assert!(state.disabled_rules.is_empty());
        assert!(state.workspace_root.is_none());
    }

    #[test]
    fn test_hover_info_table() {
        let state =
            state_with_schema("CREATE TABLE users (id INTEGER NOT NULL, name TEXT, age INTEGER);");
        let hover = state.hover_info("users").unwrap();
        assert!(hover.contains("**users** (table)"));
        assert!(hover.contains("| id | integer | NOT NULL |"));
        assert!(hover.contains("| name | text | NULL |"));
        assert!(hover.contains("| age | integer | NULL |"));
    }

    #[test]
    fn test_hover_info_view() {
        let state = state_with_schema(
            "CREATE TABLE users (id INTEGER, name TEXT);\n\
             CREATE VIEW active_users AS SELECT id, name FROM users;",
        );
        let hover = state.hover_info("active_users").unwrap();
        assert!(hover.contains("**active_users** (view)"));
        assert!(hover.contains("Columns: id, name"));
    }

    #[test]
    fn test_hover_info_column() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER NOT NULL, name TEXT);");
        let hover = state.hover_info("name").unwrap();
        assert!(hover.contains("**name** — text (nullable)"));
        assert!(hover.contains("Table: users"));
    }

    #[test]
    fn test_hover_info_column_multiple_tables() {
        let state = state_with_schema(
            "CREATE TABLE users (id INTEGER NOT NULL, name TEXT);\n\
             CREATE TABLE orders (id INTEGER NOT NULL, total NUMERIC);",
        );
        let hover = state.hover_info("id").unwrap();
        assert!(hover.contains("Table: users"));
        assert!(hover.contains("Table: orders"));
        assert!(hover.contains("---"));
    }

    #[test]
    fn test_hover_info_not_found() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER);");
        assert!(state.hover_info("nonexistent").is_none());
    }
}
