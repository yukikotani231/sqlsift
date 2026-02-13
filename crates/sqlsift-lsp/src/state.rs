use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::Url;

use sqlsift_core::schema::{Catalog, SchemaBuilder};
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
}
