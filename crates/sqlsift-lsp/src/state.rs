use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::{self, Url};

use sqlsift_core::baseline::{self, Baseline, BaselineFilter};
use sqlsift_core::ignore::IgnorePatterns;
use sqlsift_core::schema::{is_rollback_migration, Catalog, QualifiedName, SchemaBuilder};
use sqlsift_core::stack::with_analysis_stack;
use sqlsift_core::{Analyzer, Diagnostic, RuleConfig, SqlDialect, Templating};

use crate::config::Config;

pub struct ServerState {
    pub catalog: Catalog,
    pub dialect: SqlDialect,
    /// Templating of query documents (`templating` in sqlsift.toml, or Jinja in a
    /// dbt project)
    pub templating: Templating,
    /// Rule levels from sqlsift.toml
    pub rules: RuleConfig,
    pub open_documents: HashMap<Url, String>,
    pub schema_files: Vec<PathBuf>,
    /// `ignore` patterns from sqlsift.toml: matching documents get no diagnostics
    pub ignore: IgnorePatterns,
    /// `baseline` from sqlsift.toml and the directory its file names are relative to:
    /// baselined diagnostics are not shown, as in `sqlsift check`
    pub baseline: Option<(BaselineFilter, PathBuf)>,
    pub workspace_root: Option<PathBuf>,
    /// Problems found while loading sqlsift.toml, to be shown to the user
    pub config_warnings: Vec<String>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            catalog: Catalog::default(),
            dialect: SqlDialect::default(),
            templating: Templating::None,
            rules: RuleConfig::default(),
            open_documents: HashMap::new(),
            schema_files: Vec::new(),
            ignore: IgnorePatterns::default(),
            baseline: None,
            workspace_root: None,
            config_warnings: Vec::new(),
        }
    }

    /// Load configuration from sqlsift.toml and set up state
    pub fn load_config(&mut self, workspace_root: &Path) {
        self.workspace_root = Some(workspace_root.to_path_buf());
        self.templating = dbt_templating(workspace_root);

        let Some((config_path, result)) = Config::find_from_root(workspace_root) else {
            return;
        };
        let config = match result {
            Ok(config) => config,
            Err(e) => {
                self.config_warnings.push(e);
                return;
            }
        };

        // Resolve dialect
        if let Some(dialect_str) = &config.dialect {
            match dialect_str.parse() {
                Ok(d) => self.dialect = d,
                Err(e) => self
                    .config_warnings
                    .push(format!("{}: {}", config_path.display(), e)),
            }
        }

        // Templating: explicit, or Jinja when dbt_project.yml is in the workspace
        // root or next to sqlsift.toml
        let config_dir = config_path.parent().unwrap_or(workspace_root);
        match &config.templating {
            Some(templating) => match templating.parse() {
                Ok(t) => self.templating = t,
                Err(e) => self
                    .config_warnings
                    .push(format!("{}: {}", config_path.display(), e)),
            },
            None => {
                if self.templating == Templating::None {
                    self.templating = dbt_templating(config_dir);
                }
            }
        }

        // Rule levels (`disable`, `[rules]`, `[categories]`)
        let (rules, problems) = config.rule_config();
        self.rules = rules;
        self.config_warnings.extend(
            problems
                .into_iter()
                .map(|p| format!("{}: {}", config_path.display(), p)),
        );

        // Resolve schema files and ignore patterns relative to the directory
        // containing sqlsift.toml
        self.schema_files = resolve_schema_files(&config, config_dir);
        match IgnorePatterns::new(config_dir, &config.ignore) {
            Ok(ignore) => self.ignore = ignore,
            Err(e) => self
                .config_warnings
                .push(format!("{}: {}", config_path.display(), e)),
        }

        self.baseline = None;
        if let Some(path) = &config.baseline {
            let path = config_dir.join(path);
            let loaded = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read baseline {}: {}", path.display(), e))
                .and_then(|json| {
                    Baseline::from_json(&json).map_err(|e| format!("{}: {}", path.display(), e))
                });
            match loaded {
                Ok(loaded) => {
                    let dir = path.parent().unwrap_or(config_dir).to_path_buf();
                    self.baseline = Some((BaselineFilter::new(&loaded), dir));
                }
                Err(e) => self.config_warnings.push(e),
            }
        }
    }

    /// Rebuild the catalog from schema files
    pub fn rebuild_catalog(&mut self) -> Vec<String> {
        let mut builder = SchemaBuilder::with_dialect(self.dialect);
        let mut errors = Vec::new();

        for schema_file in &self.schema_files {
            match std::fs::read_to_string(schema_file) {
                Ok(content) => {
                    if let Err(diags) = with_analysis_stack(|| builder.parse(&content)) {
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

    /// Analyze a SQL document and return diagnostics. The analysis runs on a
    /// thread with a large stack, so a very long expression can't overflow the
    /// server's stack.
    pub fn analyze_document(&self, text: &str) -> Vec<Diagnostic> {
        with_analysis_stack(|| {
            let mut analyzer = Analyzer::with_dialect(&self.catalog, self.dialect)
                .with_rules(self.rules.clone())
                .with_templating(self.templating);
            analyzer.analyze(text)
        })
    }

    /// Diagnostics to show for the document at `uri`: none for ignored files
    /// (`ignore` in sqlsift.toml), and without the baselined ones
    pub fn document_diagnostics(&self, uri: &Url, text: &str) -> Vec<Diagnostic> {
        if self.is_ignored(uri) {
            return Vec::new();
        }
        let diagnostics = self.analyze_document(text);
        match (&self.baseline, uri.to_file_path()) {
            (Some((filter, dir)), Ok(path)) => {
                let key = baseline::file_key(&path, dir);
                filter.filter(&key, text, diagnostics).kept
            }
            _ => diagnostics,
        }
    }

    /// Whether the document at `uri` matches an `ignore` pattern (only `file:`
    /// URIs can match)
    pub fn is_ignored(&self, uri: &Url) -> bool {
        uri.to_file_path()
            .is_ok_and(|path| self.ignore.is_ignored(&path))
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
                let _ = writeln!(
                    md,
                    "| {} | {} | {} |",
                    col.name,
                    col.data_type.display_name(),
                    nullable
                );
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

    /// Get completion items from the schema catalog
    pub fn completion_items(&self) -> Vec<lsp_types::CompletionItem> {
        let mut items = Vec::new();

        for schema in self.catalog.schemas.values() {
            // Tables
            for table in schema.tables.values() {
                let cols: Vec<String> = table
                    .columns
                    .values()
                    .map(|c| format!("{} ({})", c.name, c.data_type.display_name()))
                    .collect();
                items.push(lsp_types::CompletionItem {
                    label: table.name.name.clone(),
                    kind: Some(lsp_types::CompletionItemKind::CLASS),
                    detail: Some("table".to_string()),
                    documentation: if cols.is_empty() {
                        None
                    } else {
                        Some(lsp_types::Documentation::String(cols.join(", ")))
                    },
                    ..Default::default()
                });

                // Columns from this table
                for col in table.columns.values() {
                    let nullable = if col.nullable { "nullable" } else { "not null" };
                    items.push(lsp_types::CompletionItem {
                        label: col.name.clone(),
                        kind: Some(lsp_types::CompletionItemKind::FIELD),
                        detail: Some(format!(
                            "{} ({}) — {}",
                            col.data_type.display_name(),
                            nullable,
                            table.name.name
                        )),
                        ..Default::default()
                    });
                }
            }

            // Views
            for view in schema.views.values() {
                let kind = if view.materialized {
                    "materialized view"
                } else {
                    "view"
                };
                items.push(lsp_types::CompletionItem {
                    label: view.name.name.clone(),
                    kind: Some(lsp_types::CompletionItemKind::INTERFACE),
                    detail: Some(kind.to_string()),
                    documentation: if view.columns.is_empty() {
                        None
                    } else {
                        Some(lsp_types::Documentation::String(view.columns.join(", ")))
                    },
                    ..Default::default()
                });
            }
        }

        items
    }
}

/// Jinja when `dir` holds a dbt project (`dbt_project.yml`), else no templating
fn dbt_templating(dir: &Path) -> Templating {
    if dir.join("dbt_project.yml").is_file() {
        Templating::Jinja
    } else {
        Templating::None
    }
}

/// Resolve schema file paths from config (handles glob patterns and schema_dir).
/// Relative paths are resolved against `base_dir` (the config file's directory).
fn resolve_schema_files(config: &Config, base_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for pattern in &config.schema {
        let abs_pattern = if Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base_dir.join(pattern).display().to_string()
        };

        if let Ok(paths) = glob::glob(&abs_pattern) {
            for path in paths.flatten() {
                files.push(path);
            }
        } else {
            // If glob fails, try as literal path
            let path = base_dir.join(pattern);
            if path.exists() {
                files.push(path);
            }
        }
    }

    if let Some(dir) = &config.schema_dir {
        let abs_dir = if Path::new(dir).is_absolute() {
            dir.clone()
        } else {
            base_dir.join(dir).display().to_string()
        };
        let pattern = format!("{abs_dir}/**/*.sql");
        if let Ok(paths) = glob::glob(&pattern) {
            // Rollback migrations (`*.down.sql`, Flyway `U*__*.sql`) are not schema
            files.extend(paths.flatten().filter(|p| !is_rollback_migration(p)));
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
    fn test_schema_dir_skips_rollback_migrations() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "sqlsift-lsp-rollbacks-{}-{nanos}",
            std::process::id()
        ));
        let migrations = root.join("migrations");
        std::fs::create_dir_all(&migrations).unwrap();
        std::fs::write(root.join("sqlsift.toml"), "schema_dir = \"migrations\"\n").unwrap();
        let files = [
            ("000001_users.down.sql", "DROP TABLE users;"),
            ("000001_users.up.sql", "CREATE TABLE users (id INT);"),
            (
                "000002_orders.sql",
                "-- migrate:up\nCREATE TABLE orders (id INT);\n-- migrate:down\nDROP TABLE orders;\n",
            ),
            ("V3__items.sql", "CREATE TABLE items (id INT);"),
            ("U3__items.sql", "DROP TABLE items;"),
        ];
        for (name, sql) in files {
            std::fs::write(migrations.join(name), sql).unwrap();
        }

        let mut state = ServerState::new();
        state.load_config(&root);
        let errors = state.rebuild_catalog();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(state.schema_files.len(), 3, "{:?}", state.schema_files);
        let diagnostics =
            state.analyze_document("SELECT u.id, o.id, i.id FROM users u, orders o, items i");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_dbt_project_enables_jinja() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sqlsift-lsp-dbt-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("schema.sql"), "CREATE TABLE users (id INT);").unwrap();
        std::fs::write(root.join("sqlsift.toml"), "schema = [\"schema.sql\"]\n").unwrap();
        let sql = "{{ config(materialized='view') }}\n\
                   SELECT u.id, s.x FROM users u JOIN {{ ref('s') }} s ON s.id = u.id";

        // Without dbt_project.yml the template is a parse error
        let mut state = ServerState::new();
        state.load_config(&root);
        state.rebuild_catalog();
        assert_eq!(state.templating, Templating::None);
        assert!(!state.analyze_document(sql).is_empty());

        std::fs::write(root.join("dbt_project.yml"), "name: demo\n").unwrap();
        let mut state = ServerState::new();
        state.load_config(&root);
        state.rebuild_catalog();
        assert_eq!(state.templating, Templating::Jinja);
        let diagnostics = state.analyze_document(sql);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        // An explicit setting wins
        std::fs::write(
            root.join("sqlsift.toml"),
            "schema = [\"schema.sql\"]\ntemplating = \"none\"\n",
        )
        .unwrap();
        let mut state = ServerState::new();
        state.load_config(&root);
        assert_eq!(state.templating, Templating::None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_is_schema_file() {
        let mut state = ServerState::new();
        state.schema_files.push(PathBuf::from("/tmp/schema.sql"));
        assert!(state.is_schema_file(Path::new("/tmp/schema.sql")));
        assert!(!state.is_schema_file(Path::new("/tmp/other.sql")));
    }

    #[test]
    fn test_is_ignored_uses_config_patterns() {
        let mut state = ServerState::new();
        let root = std::env::temp_dir().join("sqlsift-ignore-test");
        state.ignore = IgnorePatterns::new(&root, ["gen/**"]).unwrap();
        let uri = |p: &str| Url::from_file_path(root.join(p)).unwrap();
        assert!(state.is_ignored(&uri("gen/a.sql")));
        assert!(!state.is_ignored(&uri("src/a.sql")));
        assert!(!state.is_ignored(&Url::parse("untitled:Untitled-1").unwrap()));
    }

    #[test]
    fn test_new_state_defaults() {
        let state = ServerState::new();
        assert!(state.open_documents.is_empty());
        assert!(state.schema_files.is_empty());
        assert_eq!(
            state
                .rules
                .level(sqlsift_core::DiagnosticKind::ColumnNotFound),
            sqlsift_core::RuleLevel::Error
        );
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

    #[test]
    fn test_completion_items_tables_and_columns() {
        let state = state_with_schema("CREATE TABLE users (id INTEGER NOT NULL, name TEXT);");
        let items = state.completion_items();

        // Should have: 1 table + 2 columns = 3 items
        assert_eq!(items.len(), 3);

        let table_item = items.iter().find(|i| i.label == "users").unwrap();
        assert_eq!(table_item.kind, Some(lsp_types::CompletionItemKind::CLASS));
        assert_eq!(table_item.detail.as_deref(), Some("table"));

        let id_item = items.iter().find(|i| i.label == "id").unwrap();
        assert_eq!(id_item.kind, Some(lsp_types::CompletionItemKind::FIELD));
        assert!(id_item.detail.as_ref().unwrap().contains("integer"));
        assert!(id_item.detail.as_ref().unwrap().contains("users"));
    }

    #[test]
    fn test_completion_items_view() {
        let state = state_with_schema(
            "CREATE TABLE users (id INTEGER, name TEXT);\n\
             CREATE VIEW active_users AS SELECT id, name FROM users;",
        );
        let items = state.completion_items();

        let view_item = items.iter().find(|i| i.label == "active_users").unwrap();
        assert_eq!(
            view_item.kind,
            Some(lsp_types::CompletionItemKind::INTERFACE)
        );
        assert_eq!(view_item.detail.as_deref(), Some("view"));
    }

    #[test]
    fn test_completion_items_empty_catalog() {
        let state = ServerState::new();
        let items = state.completion_items();
        assert!(items.is_empty());
    }
}
