//! Detecting a project's schema, query files and SQL dialect for `sqlsift init`
//!
//! Detection only reads files: the project tree is listed once (skipping
//! dependencies, build output and hidden directories), then each tool's
//! conventions are looked up in that listing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::sqlc_config::{self, SqlcEntry};

/// Directories that are never searched: dependencies, build output, caches
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    "dbt_packages",
    "venv",
    "site-packages",
    "__pycache__",
    "coverage",
    "tmp",
    "log",
    "logs",
    "bower_components",
    "Pods",
];

/// How deep the project tree is searched
const MAX_DEPTH: usize = 10;

/// How many files are listed at most
const MAX_FILES: usize = 100_000;

/// Files larger than this are not read for detection
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Directory names of migrations
const MIGRATION_DIR_NAMES: &[&str] = &["migrations", "migration", "migrate", "db_migrations"];

/// Directory names of hand-written schema DDL
const SCHEMA_DIR_NAMES: &[&str] = &["schema", "schemas", "ddl"];

/// File names of schema dumps (`pg_dump --schema-only`, Rails `structure.sql`, ...)
const DUMP_FILE_NAMES: &[&str] = &["schema.sql", "structure.sql", "database.sql", "db.sql"];

/// Whether a file is a schema dump: named one of [`DUMP_FILE_NAMES`], or ending
/// in `_schema.sql` / `-schema.sql` / `.schema.sql` when it is not one numbered
/// step of a sequence (`34_cache_schema.sql`, `setup/34/cache_schema.sql`)
fn is_dump(path: &str) -> bool {
    let lower = file_name(path).to_ascii_lowercase();
    if DUMP_FILE_NAMES.contains(&lower.as_str()) {
        return true;
    }
    let numbered = lower.starts_with(|c: char| c.is_ascii_digit())
        || path
            .split('/')
            .rev()
            .skip(1)
            .any(|c| !c.is_empty() && c.chars().all(|c| c.is_ascii_digit()));
    !numbered
        && [
            "_schema.sql",
            "-schema.sql",
            ".schema.sql",
            "_structure.sql",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// Path components of test data, ranked below everything else
const TEST_DIR_NAMES: &[&str] = &[
    "test",
    "tests",
    "testdata",
    "fixtures",
    "spec",
    "__tests__",
    "examples",
    "example",
    "e2e",
    "testing",
];

/// Path components naming the database a schema source is written for:
/// `db/mysql/schema.sql`, `migrations/postgres/`. `other` stands for databases
/// sqlsift has no dialect for.
const DIALECT_DIR_NAMES: &[(&str, &str)] = &[
    ("postgres", "postgresql"),
    ("postgresql", "postgresql"),
    ("pg", "postgresql"),
    ("pgsql", "postgresql"),
    ("mysql", "mysql"),
    ("mariadb", "mysql"),
    ("sqlite", "sqlite"),
    ("sqlite3", "sqlite"),
    ("h2", "other"),
    ("hsqldb", "other"),
    ("derby", "other"),
    ("oracle", "other"),
    ("mssql", "other"),
    ("sqlserver", "other"),
    ("clickhouse", "other"),
];

fn dir_dialect(component: &str) -> Option<&'static str> {
    let lower = component.to_ascii_lowercase();
    DIALECT_DIR_NAMES
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, dialect)| *dialect)
}

/// The database the paths of a schema source name, if any
fn path_dialect(source: &SchemaSource) -> Option<&'static str> {
    let mut paths: Vec<&str> = source.schema_dir.iter().map(String::as_str).collect();
    paths.extend(source.schema.iter().map(String::as_str));
    paths
        .iter()
        .find_map(|p| p.split('/').rev().find_map(dir_dialect))
}

/// What the schema is read from
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSource {
    /// What was found, e.g. "Prisma migrations"
    pub label: String,
    /// `schema` entries (files or glob patterns)
    pub schema: Vec<String>,
    /// `schema_dir`
    pub schema_dir: Option<String>,
    /// Number of SQL files it covers
    pub sql_files: usize,
}

impl SchemaSource {
    /// The paths of this source, for messages
    pub fn paths(&self) -> String {
        let mut paths: Vec<String> = self.schema_dir.iter().cloned().collect();
        paths.extend(self.schema.iter().cloned());
        paths.join(", ")
    }
}

/// A group of query files
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryGroup {
    /// What was found, e.g. "SQL files" or "sql`...` templates in TypeScript"
    pub label: String,
    /// `files` patterns
    pub patterns: Vec<String>,
    /// Number of files found
    pub count: usize,
}

/// The SQL dialect and what it was detected from
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialectHint {
    pub dialect: &'static str,
    pub source: String,
}

/// A dbt project
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbtProject {
    /// Directory of `dbt_project.yml` ("" for the project root)
    pub dir: String,
    /// Whether `target/catalog.json` exists
    pub has_catalog: bool,
}

/// Everything `sqlsift init` found in a project
#[derive(Debug, Default, Clone)]
pub struct Detection {
    /// The schema source to use
    pub schema: Option<SchemaSource>,
    /// Other schema sources found
    pub other_schemas: Vec<SchemaSource>,
    /// Query files
    pub queries: Vec<QueryGroup>,
    /// Query files to skip
    pub ignore: Vec<String>,
    /// The dialect, from the most specific hint
    pub dialect: Option<DialectHint>,
    /// Hints that disagree with `dialect`
    pub other_dialects: Vec<DialectHint>,
    /// `embedded_sql_tags`, when the default (`["sql"]`) is not enough
    pub embedded_sql_tags: Option<Vec<String>>,
    pub dbt: Option<DbtProject>,
    /// Whether a sqlc configuration was found
    pub sqlc: bool,
    /// Advice printed and written as comments
    pub notes: Vec<String>,
}

/// The files of a project, relative to its root, with `/` separators
struct Tree {
    root: PathBuf,
    files: Vec<String>,
}

impl Tree {
    fn list(root: &Path) -> Self {
        let mut files = Vec::new();
        let mut stack = vec![(root.to_path_buf(), String::new(), 0)];
        while let Some((dir, rel, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    let hidden = name.starts_with('.');
                    if !hidden && !SKIPPED_DIRS.contains(&name.as_str()) && depth < MAX_DEPTH {
                        stack.push((entry.path(), path, depth + 1));
                    }
                } else if file_type.is_file() && files.len() < MAX_FILES {
                    files.push(path);
                }
            }
        }
        files.sort();
        Self {
            root: root.to_path_buf(),
            files,
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.files
            .binary_search_by(|f| f.as_str().cmp(path))
            .is_ok()
    }

    fn read(&self, path: &str) -> Option<String> {
        let full = self.root.join(path);
        let size = std::fs::metadata(&full).ok()?.len();
        if size > MAX_READ_BYTES {
            return None;
        }
        std::fs::read_to_string(full).ok()
    }

    /// Files under `dir` (every file when `dir` is empty)
    fn under<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = &'a String> + 'a {
        self.files.iter().filter(move |f| is_under(f, dir))
    }

    /// Files named `name` (at any depth), shallowest first
    fn named(&self, names: &[&str]) -> Vec<&String> {
        let mut found: Vec<&String> = self
            .files
            .iter()
            .filter(|f| names.contains(&file_name(f)))
            .collect();
        found.sort_by_key(|f| (depth(f), (*f).clone()));
        found
    }
}

/// Whether `path` is `dir` or below it (everything is below "")
fn is_under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path == dir
        || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

fn join(dir: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    let path = path.trim_end_matches('/');
    let joined = if dir.is_empty() || path.starts_with('/') {
        path.to_string()
    } else if path.is_empty() || path == "." {
        dir.to_string()
    } else {
        format!("{dir}/{path}")
    };
    normalize(&joined)
}

/// `a/./b/../c` → `a/c`
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." if !parts.is_empty() || !path.starts_with('/') => {}
            ".." if parts.last().is_some_and(|p| *p != ".." && !p.is_empty()) => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if path.starts_with('/') && !joined.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn is_sql(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".sql")
}

/// Whether a path is in test data: a component in [`TEST_DIR_NAMES`], or one like
/// `integration_test`, `e2e-tests` or `test_data`
fn in_test_dir(path: &str) -> bool {
    path.split('/').any(|c| {
        let lower = c.to_ascii_lowercase();
        TEST_DIR_NAMES.contains(&lower.as_str())
            || ["_test", "_tests", "-test", "-tests"]
                .iter()
                .any(|suffix| lower.ends_with(suffix))
            || ["test_", "test-", "tests_", "tests-"]
                .iter()
                .any(|prefix| lower.starts_with(prefix))
    })
}

/// Number of tables SQL text creates
fn count_tables(sql: &str) -> usize {
    sql.to_ascii_lowercase().matches("create table").count()
}

/// Whether SQL text defines tables (so a directory of it is a schema source)
fn has_ddl(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.contains("create table") || lower.contains("alter table") || lower.contains("create view")
}

/// Detect the layout of the project at `root`
pub fn detect(root: &Path) -> Detection {
    let tree = Tree::list(root);
    let mut detection = Detection::default();
    let mut dialects: Vec<DialectHint> = Vec::new();

    // SQL files that belong to a schema source or a dbt project, never query files
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    let mut query_sql: BTreeSet<String> = BTreeSet::new();
    let mut queries: Vec<QueryGroup> = Vec::new();

    // sqlc: its configuration names the schema, the queries and the engine
    let sqlc_file = tree
        .named(&["sqlc.yaml", "sqlc.yml", "sqlc.json"])
        .into_iter()
        .find(|f| !in_test_dir(f))
        .cloned()
        .unwrap_or_default();
    let mut sqlc_entries: Vec<(String, SqlcEntry)> = Vec::new();
    if let Some(text) = (!sqlc_file.is_empty())
        .then(|| tree.read(&sqlc_file))
        .flatten()
    {
        let json = Path::new(&sqlc_file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        for entry in sqlc_config::parse(&text, json) {
            sqlc_entries.push((sqlc_file.clone(), entry));
        }
    }
    // (rank, tables created, source)
    let mut candidates: Vec<(u8, usize, SchemaSource)> = Vec::new();
    if !sqlc_entries.is_empty() {
        detection.sqlc = true;
        let mut schema_dirs: Vec<String> = Vec::new();
        let mut schema_files: Vec<String> = Vec::new();
        let mut query_patterns: Vec<String> = Vec::new();
        let mut query_count = 0;
        for (config, entry) in &sqlc_entries {
            let base = parent(config);
            if let Some(engine) = &entry.engine {
                if let Some(dialect) = engine_dialect(engine) {
                    dialects.push(DialectHint {
                        dialect,
                        source: format!("{config}: engine \"{engine}\""),
                    });
                }
            }
            for schema in &entry.schema {
                let path = join(base, schema);
                if tree.exists(&path) {
                    if !schema_files.contains(&path) {
                        schema_files.push(path);
                    }
                } else if !schema_dirs.contains(&path) {
                    schema_dirs.push(path);
                }
            }
            for query in &entry.queries {
                let path = join(base, query);
                let pattern = if tree.exists(&path) {
                    query_count += 1;
                    path
                } else {
                    query_count += tree.under(&path).filter(|f| is_sql(f)).count();
                    format!("{path}/*.sql")
                };
                if !query_patterns.contains(&pattern) {
                    query_patterns.push(pattern);
                }
            }
        }
        let sql_files = schema_files.len()
            + schema_dirs
                .iter()
                .map(|d| tree.under(d).filter(|f| is_sql(f)).count())
                .sum::<usize>();
        // One directory is `schema_dir` (rollback migrations are skipped); others
        // are glob patterns
        let mut dirs = schema_dirs.iter();
        let schema_dir = dirs.next().cloned();
        schema_files.extend(dirs.map(|d| format!("{d}/*.sql")));
        if schema_dir.is_some() || !schema_files.is_empty() {
            candidates.push((
                0,
                0,
                SchemaSource {
                    label: format!("sqlc schema ({sqlc_file})"),
                    schema: schema_files,
                    schema_dir,
                    sql_files,
                },
            ));
        }
        if !query_patterns.is_empty() {
            queries.push(QueryGroup {
                label: format!("sqlc queries ({sqlc_file})"),
                patterns: query_patterns,
                count: query_count,
            });
        }
    }

    // Prisma
    let prisma_schema = tree
        .named(&["schema.prisma"])
        .into_iter()
        .find(|f| !in_test_dir(f))
        .cloned()
        .or_else(|| {
            // Multi-file schemas: prisma/schema/*.prisma
            tree.files
                .iter()
                .find(|f| f.ends_with(".prisma") && !in_test_dir(f))
                .cloned()
        });
    let mut prisma = false;
    if let Some(schema) = &prisma_schema {
        prisma = true;
        if let Some(text) = tree.read(schema) {
            if let Some(provider) = prisma_provider(&text) {
                match provider_dialect(&provider) {
                    Some(dialect) => dialects.push(DialectHint {
                        dialect,
                        source: format!("{schema}: provider = \"{provider}\""),
                    }),
                    None => detection.notes.push(format!(
                        "{schema} uses provider \"{provider}\", which sqlsift does not support"
                    )),
                }
            }
        }
    }

    // Migration directories: every directory with a conventional name (or with
    // Flyway-named files) that holds SQL defining tables
    let mut migration_dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in tree.files.iter().filter(|f| is_sql(f)) {
        let components: Vec<&str> = file.split('/').collect();
        let named = components[..components.len() - 1]
            .iter()
            .rposition(|c| MIGRATION_DIR_NAMES.contains(c) || *c == "drizzle")
            .map(|i| components[..=i].join("/"));
        let dir = named.or_else(|| is_flyway(file_name(file)).then(|| parent(file).to_string()));
        if let Some(dir) = dir {
            migration_dirs.entry(dir).or_default().push(file.clone());
        }
    }
    // A drizzle.config output directory with another name
    for config in tree.named(&[
        "drizzle.config.ts",
        "drizzle.config.js",
        "drizzle.config.mjs",
        "drizzle.config.json",
    ]) {
        let Some(text) = tree.read(config) else {
            continue;
        };
        if let Some(dialect) = drizzle_dialect(&text) {
            dialects.push(DialectHint {
                dialect,
                source: format!("{config}: dialect"),
            });
        }
        if let Some(out) = config_string(&text, "out") {
            let dir = join(parent(config), &out);
            let files: Vec<String> = tree.under(&dir).filter(|f| is_sql(f)).cloned().collect();
            if !files.is_empty() {
                migration_dirs.insert(dir, files);
            }
        }
    }
    // Nested migration directories are part of the outer one
    let dirs: Vec<String> = migration_dirs.keys().cloned().collect();
    for dir in &dirs {
        if dirs.iter().any(|d| d != dir && is_under(dir, d)) {
            if let Some(files) = migration_dirs.remove(dir) {
                if let Some(outer) = dirs.iter().find(|d| *d != dir && is_under(dir, d)) {
                    migration_dirs
                        .entry(outer.clone())
                        .or_default()
                        .extend(files);
                }
            }
        }
    }
    // A migrations directory with one subdirectory per database
    // (`migrations/mysql/`, `migrations/postgres/`) is one source per database
    let dirs: Vec<String> = migration_dirs.keys().cloned().collect();
    for dir in dirs {
        let Some(files) = migration_dirs.remove(&dir) else {
            continue;
        };
        let mut rest = Vec::new();
        let mut per_dialect: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in files {
            let below = file[dir.len()..].trim_start_matches('/');
            match below.split_once('/') {
                Some((sub, _)) if dir_dialect(sub).is_some() => per_dialect
                    .entry(format!("{dir}/{sub}"))
                    .or_default()
                    .push(file),
                _ => rest.push(file),
            }
        }
        if !rest.is_empty() {
            migration_dirs.insert(dir, rest);
        }
        migration_dirs.extend(per_dialect);
    }
    for (dir, files) in &migration_dirs {
        let texts: Vec<String> = files.iter().filter_map(|f| tree.read(f)).collect();
        if !texts.iter().any(|t| has_ddl(t)) {
            continue;
        }
        let (label, rank) = migration_label(dir, files, &texts);
        if label == "Supabase migrations" {
            dialects.push(DialectHint {
                dialect: "postgresql",
                source: format!("{dir} (Supabase)"),
            });
        }
        let rank = if in_test_dir(dir) { rank + 10 } else { rank };
        candidates.push((
            rank,
            texts.iter().map(|t| count_tables(t)).sum(),
            SchemaSource {
                label: label.to_string(),
                schema: Vec::new(),
                schema_dir: Some(dir.clone()),
                sql_files: files.len(),
            },
        ));
    }

    // Schema dumps and directories of hand-written DDL
    let mut dumps: Vec<&String> = tree.files.iter().filter(|f| is_dump(f)).collect();
    dumps.sort_by_key(|f| (depth(f), (*f).clone()));
    for dump in dumps {
        if migration_dirs.keys().any(|d| is_under(dump, d)) {
            continue;
        }
        let Some(text) = tree.read(dump) else {
            continue;
        };
        if !has_ddl(&text) {
            continue;
        }
        let rails = dump == "db/structure.sql" || dump.ends_with("/db/structure.sql");
        let ecto = dump == "priv/repo/structure.sql" || dump.ends_with("/priv/repo/structure.sql");
        let label = if rails {
            "Rails structure.sql"
        } else if ecto {
            "Ecto structure.sql"
        } else {
            "schema dump"
        };
        let rank = if rails || ecto { 1 } else { 3 } + if in_test_dir(dump) { 10 } else { 0 };
        candidates.push((
            rank,
            count_tables(&text),
            SchemaSource {
                label: label.to_string(),
                schema: vec![dump.clone()],
                schema_dir: None,
                sql_files: 1,
            },
        ));
    }
    let mut schema_dirs: BTreeSet<String> = BTreeSet::new();
    for file in tree.files.iter().filter(|f| is_sql(f)) {
        let dir = parent(file);
        if SCHEMA_DIR_NAMES.contains(&file_name(dir)) && !migration_dirs.contains_key(dir) {
            schema_dirs.insert(dir.to_string());
        }
    }
    for dir in schema_dirs {
        let files: Vec<&String> = tree.under(&dir).filter(|f| is_sql(f)).collect();
        let texts: Vec<String> = files.iter().filter_map(|f| tree.read(f)).collect();
        if !texts.iter().any(|t| has_ddl(t)) {
            continue;
        }
        let rank = if in_test_dir(&dir) { 13 } else { 3 };
        candidates.push((
            rank,
            texts.iter().map(|t| count_tables(t)).sum(),
            SchemaSource {
                label: "schema directory".to_string(),
                schema: Vec::new(),
                schema_dir: Some(dir.clone()),
                sql_files: files.len(),
            },
        ));
    }

    // dbt
    let dbt_yml = tree
        .named(&["dbt_project.yml"])
        .into_iter()
        .find(|f| !in_test_dir(f))
        .cloned();
    if let Some(yml) = &dbt_yml {
        let dir = parent(yml).to_string();
        let has_catalog = tree.root.join(join(&dir, "target/catalog.json")).is_file();
        let text = tree.read(yml).unwrap_or_default();
        let model_paths = yaml_list(&text, "model-paths")
            .or_else(|| yaml_list(&text, "source-paths"))
            .unwrap_or_else(|| vec!["models".to_string()]);
        let mut patterns = Vec::new();
        let mut count = 0;
        for path in &model_paths {
            let models = join(&dir, path);
            count += tree.under(&models).filter(|f| is_sql(f)).count();
            patterns.push(format!("{models}/**/*.sql"));
        }
        // Every SQL file of the dbt project (models, macros, tests, ...) is
        // handled here
        excluded.extend(tree.under(&dir).filter(|f| is_sql(f)).cloned());
        if count > 0 {
            queries.push(QueryGroup {
                label: "dbt models".to_string(),
                patterns,
                count,
            });
        }
        if let Some(dialect) = dbt_dialect(&tree, &dir) {
            dialects.push(dialect);
        }
        detection.dbt = Some(DbtProject { dir, has_catalog });
    }

    // Dialect hints that don't depend on the schema source, to prefer a source
    // written for that database
    let project_dialects: Vec<DialectHint> = env_dialects(&tree)
        .into_iter()
        .chain(compose_dialects(&tree))
        .chain(dependency_dialects(&tree))
        .collect();
    let preferred = dialects
        .first()
        .or(project_dialects.first())
        .map(|hint| hint.dialect);
    let other_database = |source: &SchemaSource| {
        preferred.is_some() && path_dialect(source).is_some_and(|d| Some(d) != preferred)
    };

    // Pick the schema source: lowest rank, not written for another database,
    // then most tables, then shallowest, then most files
    candidates.sort_by(|(ra, ta, a), (rb, tb, b)| {
        ra.cmp(rb)
            .then_with(|| other_database(a).cmp(&other_database(b)))
            .then_with(|| tb.cmp(ta))
            .then_with(|| depth(&a.paths()).cmp(&depth(&b.paths())))
            .then_with(|| b.sql_files.cmp(&a.sql_files))
            .then_with(|| a.paths().cmp(&b.paths()))
    });
    for (_, _, source) in &candidates {
        if let Some(dir) = &source.schema_dir {
            excluded.extend(tree.under(dir).filter(|f| is_sql(f)).cloned());
        }
        for file in &source.schema {
            excluded.insert(file.clone());
        }
    }
    // The same source found twice (a sqlc schema directory is also a migrations
    // directory) is listed once
    let mut seen_paths = BTreeSet::new();
    let mut candidates = candidates
        .into_iter()
        .map(|(_, _, s)| s)
        .filter(|s| seen_paths.insert(s.paths()));
    detection.schema = candidates.next();
    detection.other_schemas = candidates.collect();
    if let Some(schema) = &detection.schema {
        if schema.label.starts_with("Rails") {
            if let Some(dialect) = rails_dialect(&tree) {
                dialects.push(dialect);
            }
        }
    }

    // Query SQL files: those of sqlc, else every other SQL file
    if !detection.sqlc {
        let sql: Vec<&String> = tree
            .files
            .iter()
            .filter(|f| is_sql(f) && !excluded.contains(*f))
            .collect();
        query_sql.extend(sql.into_iter().cloned());
        let all_sql: Vec<&String> = tree.files.iter().filter(|f| is_sql(f)).collect();
        let (groups, ignore) = group_files(&query_sql, &excluded, &all_sql);
        for (pattern, count) in groups {
            let label = if prisma
                && (pattern.starts_with("prisma/sql/") || pattern.contains("/prisma/sql/"))
            {
                "Prisma TypedSQL"
            } else {
                "SQL files"
            };
            match queries.iter_mut().find(|g| g.label == label) {
                Some(group) => {
                    group.patterns.push(pattern);
                    group.count += count;
                }
                None => queries.push(QueryGroup {
                    label: label.to_string(),
                    patterns: vec![pattern],
                    count,
                }),
            }
        }
        detection.ignore = ignore;
    }

    // SQL in TypeScript / JavaScript tagged templates
    let mut prisma_raw = false;
    let mut embedded: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in &tree.files {
        let path = Path::new(file);
        if !sqlsift_core::embedded::is_embedded_sql_file(path) {
            continue;
        }
        let Some(text) = tree.read(file) else {
            continue;
        };
        let tags = template_tags(&text);
        if tags.sql || tags.prisma_raw {
            prisma_raw |= tags.prisma_raw;
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default();
            embedded.entry(ext).or_default().push(file.clone());
        }
    }
    let mut embedded_count = 0;
    let mut embedded_patterns = Vec::new();
    let mut extensions = Vec::new();
    for (ext, files) in &embedded {
        embedded_count += files.len();
        extensions.push(format!(".{ext}"));
        let set: BTreeSet<String> = files.iter().cloned().collect();
        for (pattern, _) in group_by_top_dir(&set, ext) {
            embedded_patterns.push(pattern);
        }
    }
    if embedded_count > 0 {
        let tags = if prisma_raw {
            "sql`...` / $queryRaw`...` templates"
        } else {
            "sql`...` templates"
        };
        queries.push(QueryGroup {
            label: format!("{tags} in {}", extensions.join(", ")),
            patterns: embedded_patterns,
            count: embedded_count,
        });
    }

    // SQL in Rust: sqlx's query macros and `sqlx::` functions
    let rust: BTreeSet<String> = tree
        .files
        .iter()
        .filter(|file| sqlsift_core::sqlx::is_rust_file(Path::new(file)))
        .filter(|file| tree.read(file).is_some_and(|text| uses_sqlx_queries(&text)))
        .cloned()
        .collect();
    if !rust.is_empty() {
        queries.push(QueryGroup {
            label: "sqlx query!(\"...\") in .rs".to_string(),
            patterns: group_by_top_dir(&rust, "rs")
                .into_iter()
                .map(|(pattern, _)| pattern)
                .collect(),
            count: rust.len(),
        });
    }

    // SQL in Python: string literals passed to database calls
    let python: BTreeSet<String> = tree
        .files
        .iter()
        .filter(|file| sqlsift_core::python::is_python_file(Path::new(file)))
        .filter(|file| {
            tree.read(file)
                .is_some_and(|text| uses_python_queries(&text))
        })
        .cloned()
        .collect();
    if !python.is_empty() {
        queries.push(QueryGroup {
            label: "cursor.execute(\"...\") in .py".to_string(),
            patterns: group_by_top_dir(&python, "py")
                .into_iter()
                .map(|(pattern, _)| pattern)
                .collect(),
            count: python.len(),
        });
    }

    if prisma_raw {
        detection.embedded_sql_tags = Some(
            ["sql", "$queryRaw", "$executeRaw"]
                .iter()
                .map(ToString::to_string)
                .collect(),
        );
    }
    detection.queries = queries;

    // Dialect hints from the rest of the project, least specific last
    dialects.extend(project_dialects);
    if let Some(schema) = &detection.schema {
        if let Some(dialect) = path_dialect(schema).filter(|d| *d != "other") {
            dialects.push(DialectHint {
                dialect,
                source: format!("directory of {}", schema.paths()),
            });
        }
        if let Some(hint) = sniff_dialect(&tree, schema) {
            dialects.push(hint);
        }
    }
    let mut dialects = dialects.into_iter();
    detection.dialect = dialects.next();
    if let Some(chosen) = &detection.dialect {
        let mut seen = BTreeSet::new();
        detection.other_dialects = dialects
            .filter(|d| d.dialect != chosen.dialect && seen.insert(d.dialect))
            .collect();
    }

    // A schema written for another database than the dialect
    if let (Some(schema), Some(chosen)) = (&detection.schema, &detection.dialect) {
        match path_dialect(schema) {
            Some("other") => detection.notes.push(format!(
                "{} looks written for a database sqlsift has no dialect for: \
                 point `schema` at the {} schema if the project has one",
                schema.paths(),
                chosen.dialect
            )),
            Some(dialect) if dialect != chosen.dialect => detection.notes.push(format!(
                "{} looks written for {dialect}, but the dialect is {}: \
                 point `schema` at the {} schema if the project has one",
                schema.paths(),
                chosen.dialect,
                chosen.dialect
            )),
            _ => {}
        }
    }

    add_notes(&tree, &mut detection, prisma_schema.as_deref());
    detection
}

/// Advice for layouts whose schema sqlsift can't read directly
fn add_notes(tree: &Tree, detection: &mut Detection, prisma_schema: Option<&str>) {
    if detection.schema.is_some() {
        return;
    }
    if let Some(schema) = prisma_schema {
        detection.notes.push(format!(
            "{schema} found but no prisma/migrations: create a SQL schema with \
             `npx prisma migrate diff --from-empty --to-schema-datamodel {schema} --script > schema.sql` \
             and set `schema = [\"schema.sql\"]`"
        ));
    } else if tree.exists("db/schema.rb") {
        detection.notes.push(
            "db/schema.rb is Ruby: set `config.active_record.schema_format = :sql` so Rails \
             writes db/structure.sql, then set `schema = [\"db/structure.sql\"]`"
                .to_string(),
        );
    } else if tree.files.iter().any(|f| {
        let name = file_name(f);
        name.starts_with("knexfile.") || name == ".sequelizerc"
    }) {
        detection.notes.push(
            "the migrations are JavaScript: dump the schema with `pg_dump --schema-only` \
             (or `mysqldump --no-data`) to schema.sql and set `schema = [\"schema.sql\"]`"
                .to_string(),
        );
    } else if tree
        .files
        .iter()
        .any(|f| file_name(f).starts_with("drizzle.config."))
    {
        detection.notes.push(
            "drizzle.config found but no SQL migrations: run `npx drizzle-kit generate` \
             and point `schema_dir` at its output directory"
                .to_string(),
        );
    } else if detection.dbt.as_ref().is_some_and(|d| !d.has_catalog) {
        // dbt projects read the columns of models from the catalog
    } else {
        detection.notes.push(
            "no schema found: set `schema` (DDL files, e.g. a `pg_dump --schema-only` dump) \
             or `schema_dir` (a directory of migrations) in sqlsift.toml"
                .to_string(),
        );
    }
}

/// The kind of a migrations directory and its rank (lower is preferred)
fn migration_label(dir: &str, files: &[String], texts: &[String]) -> (&'static str, u8) {
    if dir == "prisma/migrations" || dir.ends_with("/prisma/migrations") {
        return ("Prisma migrations", 1);
    }
    if dir == "supabase/migrations" || dir.ends_with("/supabase/migrations") {
        return ("Supabase migrations", 1);
    }
    if file_name(dir) == "drizzle"
        || files
            .iter()
            .any(|f| f.ends_with("meta/_journal.json") || f.contains("/meta/"))
        || texts.iter().any(|t| t.contains("--> statement-breakpoint"))
    {
        return ("Drizzle migrations", 1);
    }
    if files.iter().any(|f| is_flyway(file_name(f))) {
        return ("Flyway migrations", 2);
    }
    if texts.iter().any(|t| t.contains("-- +goose")) {
        return ("goose migrations", 2);
    }
    if texts.iter().any(|t| t.contains("migrate:up")) {
        return ("dbmate migrations", 2);
    }
    if files
        .iter()
        .any(|f| f.to_ascii_lowercase().ends_with(".up.sql"))
    {
        return ("golang-migrate / sqlx migrations", 2);
    }
    ("SQL migrations", 4)
}

/// Flyway versioned (`V1__x.sql`) or repeatable (`R__x.sql`) migration
fn is_flyway(name: &str) -> bool {
    let Some((prefix, _)) = name.split_once("__") else {
        return false;
    };
    if prefix == "R" {
        return true;
    }
    prefix.strip_prefix('V').is_some_and(|v| {
        v.starts_with(|c: char| c.is_ascii_digit())
            && v.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '_')
    })
}

/// Group query files into `files` patterns, keeping clear of schema sources.
///
/// Each file is covered by the highest directory above it (below the project
/// root) that holds no excluded SQL file, as `dir/**/*.sql`. Files with no such
/// directory are listed one by one, or as `dir/*.sql` plus an `ignore` entry for
/// each excluded file there when a directory has many. Returns the patterns
/// with their file counts, and the ignore patterns.
fn group_files(
    files: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
    all_sql: &[&String],
) -> (Vec<(String, usize)>, Vec<String>) {
    let clean = |dir: &str| !excluded.iter().any(|e| is_under(e, dir));
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut loose: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        let components: Vec<&str> = file.split('/').collect();
        let dirs = &components[..components.len() - 1];
        let top = (1..=dirs.len())
            .map(|n| dirs[..n].join("/"))
            .find(|dir| clean(dir));
        match top {
            Some(dir) => *groups.entry(format!("{dir}/**/*.sql")).or_default() += 1,
            None => loose
                .entry(parent(file).to_string())
                .or_default()
                .push(file.clone()),
        }
    }
    let mut ignore = Vec::new();
    for (dir, files) in loose {
        if files.len() > 3 {
            let pattern = if dir.is_empty() {
                "*.sql".to_string()
            } else {
                format!("{dir}/*.sql")
            };
            for other in all_sql {
                if parent(other) == dir && excluded.contains(*other) {
                    ignore.push((*other).clone());
                }
            }
            groups.insert(pattern, files.len());
        } else {
            for file in files {
                groups.insert(file, 1);
            }
        }
    }
    (groups.into_iter().collect(), ignore)
}

/// `top/**/*.ext` for the top-level directory of each file (files in the root
/// are listed by name)
fn group_by_top_dir(files: &BTreeSet<String>, ext: &str) -> Vec<(String, usize)> {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        let pattern = match file.split_once('/') {
            Some((top, _)) => format!("{top}/**/*.{ext}"),
            None => file.clone(),
        };
        *groups.entry(pattern).or_default() += 1;
    }
    groups.into_iter().collect()
}

/// Which tagged templates a TypeScript / JavaScript file uses
#[derive(Default)]
struct TemplateTags {
    /// `sql\`...\``, `db.sql\`...\``, `` sql<T>`...` ``, `sql.type(...)\`...\``
    sql: bool,
    /// `prisma.$queryRaw\`...\`` / `$executeRaw`
    prisma_raw: bool,
}

fn template_tags(text: &str) -> TemplateTags {
    let mut tags = TemplateTags::default();
    for (i, _) in text.match_indices("sql") {
        let before = text[..i].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            continue;
        }
        let after = &text[i + 3..];
        if after.starts_with('`')
            || after.starts_with(".type(")
            || after.starts_with(".typeAlias(")
            || (after.starts_with('<')
                && after
                    .find('`')
                    .is_some_and(|end| !after[..end].contains([';', '\n', '(', '='])))
        {
            tags.sql = true;
            break;
        }
    }
    tags.prisma_raw = ["$queryRaw`", "$queryRaw<", "$executeRaw`", "$executeRaw<"]
        .iter()
        .any(|tag| text.contains(tag));
    tags
}

/// Whether Rust code passes SQL to one of sqlx's query macros (`query!("...")`,
/// `sqlx::query_as!(T, "...")`) or functions (`sqlx::query("...")`); not
/// `query_file!`, whose files are found as `.sql` files
fn uses_sqlx_queries(text: &str) -> bool {
    sqlsift_core::sqlx::DEFAULT_MACROS.iter().any(|name| {
        text.match_indices(&format!("{name}!")).any(|(i, m)| {
            let before = text[..i].chars().next_back();
            let after = text[i + m.len()..].trim_start();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && after.starts_with(['(', '[', '{'])
        }) || text.contains(&format!("sqlx::{name}("))
            || text.contains(&format!("sqlx::{name}::<"))
    })
}

/// Whether Python code passes SQL to a database call that `sqlsift check`
/// extracts (`cur.execute("SELECT ...")`, `text("SELECT ...")`)
fn uses_python_queries(text: &str) -> bool {
    sqlsift_core::python::has_sql(text, sqlsift_core::python::DEFAULT_FUNCTIONS)
}

/// The dialect of a sqlc `engine`
fn engine_dialect(engine: &str) -> Option<&'static str> {
    match engine.to_ascii_lowercase().as_str() {
        "postgresql" | "postgres" => Some("postgresql"),
        "mysql" => Some("mysql"),
        "sqlite" => Some("sqlite"),
        _ => None,
    }
}

/// The dialect of a Prisma datasource `provider`
fn provider_dialect(provider: &str) -> Option<&'static str> {
    match provider {
        "postgresql" | "postgres" | "cockroachdb" => Some("postgresql"),
        "mysql" => Some("mysql"),
        "sqlite" => Some("sqlite"),
        _ => None,
    }
}

/// `provider` of the `datasource` block of a Prisma schema
fn prisma_provider(schema: &str) -> Option<String> {
    let start = schema.find("datasource")?;
    let block = &schema[start..];
    let end = block.find('}').unwrap_or(block.len());
    config_string(&block[..end], "provider")
}

/// The string value of `key = "value"` or `key: "value"` in a config file
fn config_string(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix(['=', ':']) else {
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next()?;
        if !matches!(quote, '"' | '\'' | '`') {
            continue;
        }
        let value = &rest[1..];
        let end = value.find(quote)?;
        return Some(value[..end].to_string());
    }
    None
}

/// The dialect of a drizzle.config (`dialect: "postgresql"`, or `driver` in old
/// versions)
fn drizzle_dialect(text: &str) -> Option<&'static str> {
    let value = config_string(text, "dialect").or_else(|| config_string(text, "driver"))?;
    match value.as_str() {
        "postgresql" | "pg" | "postgres" => Some("postgresql"),
        "mysql" | "mysql2" | "singlestore" => Some("mysql"),
        "sqlite" | "turso" | "better-sqlite" | "libsql" | "d1" | "d1-http" => Some("sqlite"),
        _ => None,
    }
}

/// A YAML `key: [a, b]` or block list value, or `key: value`
fn yaml_list(text: &str, key: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix(key).and_then(|r| r.strip_prefix(':')) else {
            continue;
        };
        let rest = rest.split(" #").next().unwrap_or("").trim();
        let unquote = |s: &str| s.trim().trim_matches(['"', '\'']).to_string();
        if let Some(list) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            return Some(
                list.split(',')
                    .map(unquote)
                    .filter(|s| !s.is_empty())
                    .collect(),
            );
        }
        if !rest.is_empty() {
            return Some(vec![unquote(rest)]);
        }
        let items: Vec<String> = lines
            .by_ref()
            .map_while(|l| l.trim().strip_prefix('-').map(unquote))
            .filter(|s| !s.is_empty())
            .collect();
        return (!items.is_empty()).then_some(items);
    }
    None
}

/// The dialect of a dbt project: the `type` of a profiles.yml in the project, or
/// the dbt adapter in its Python requirements
fn dbt_dialect(tree: &Tree, dir: &str) -> Option<DialectHint> {
    let adapter = |name: &str| -> Option<&'static str> {
        match name {
            "postgres" => Some("postgresql"),
            "snowflake" => Some("snowflake"),
            "bigquery" => Some("bigquery"),
            "redshift" => Some("redshift"),
            "databricks" | "spark" => Some("databricks"),
            "mysql" => Some("mysql"),
            "sqlite" => Some("sqlite"),
            _ => None,
        }
    };
    let profiles = join(dir, "profiles.yml");
    if let Some(text) = tree.read(&profiles) {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("type:") {
                let value = value.trim().trim_matches(['"', '\'']);
                if let Some(dialect) = adapter(value) {
                    return Some(DialectHint {
                        dialect,
                        source: format!("{profiles}: type: {value}"),
                    });
                }
            }
        }
    }
    let requirement_files = tree.files.iter().filter(|f| {
        let name = file_name(f);
        (name.starts_with("requirements")
            && Path::new(name).extension().is_some_and(|e| e == "txt"))
            || name == "pyproject.toml"
            || name == "Pipfile"
    });
    for file in requirement_files {
        let Some(text) = tree.read(file) else {
            continue;
        };
        for (i, _) in text.match_indices("dbt-") {
            let name: String = text[i + 4..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            if let Some(dialect) = adapter(&name) {
                return Some(DialectHint {
                    dialect,
                    source: format!("{file}: dbt-{name}"),
                });
            }
        }
    }
    None
}

/// The dialect of Rails' config/database.yml `adapter`
fn rails_dialect(tree: &Tree) -> Option<DialectHint> {
    let text = tree.read("config/database.yml")?;
    for line in text.lines() {
        let Some(value) = line.trim().strip_prefix("adapter:") else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        let dialect = match value {
            "postgresql" | "postgis" => "postgresql",
            "mysql2" | "trilogy" => "mysql",
            "sqlite3" => "sqlite",
            _ => continue,
        };
        return Some(DialectHint {
            dialect,
            source: format!("config/database.yml: adapter: {value}"),
        });
    }
    None
}

/// The dialect of a database URL's scheme
fn url_dialect(url: &str) -> Option<&'static str> {
    let url = url.trim().trim_matches(['"', '\'']);
    let scheme = url.split(':').next()?.to_ascii_lowercase();
    match scheme.as_str() {
        "postgres" | "postgresql" | "postgis" => Some("postgresql"),
        "mysql" | "mysql2" | "mariadb" => Some("mysql"),
        "sqlite" | "sqlite3" | "file" => Some("sqlite"),
        _ => None,
    }
}

/// Dialects of `DATABASE_URL`-style variables in example env files (their values
/// are never printed)
fn env_dialects(tree: &Tree) -> Vec<DialectHint> {
    let mut hints = Vec::new();
    for name in [
        ".env.example",
        ".env.sample",
        ".env.template",
        ".env.dist",
        ".env",
    ] {
        let Some(text) = tree
            .root
            .join(name)
            .is_file()
            .then(|| tree.read(name))
            .flatten()
        else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim().trim_start_matches("export ");
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if !key.contains("DATABASE_URL") && !key.contains("DB_URL") {
                continue;
            }
            if let Some(dialect) = url_dialect(value) {
                hints.push(DialectHint {
                    dialect,
                    source: format!("{name}: {}", key.trim()),
                });
                break;
            }
        }
    }
    hints
}

/// Dialects of database images in Docker Compose files
fn compose_dialects(tree: &Tree) -> Vec<DialectHint> {
    let mut hints = Vec::new();
    let files = tree.named(&[
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ]);
    for file in files.into_iter().filter(|f| depth(f) <= 1) {
        let Some(text) = tree.read(file) else {
            continue;
        };
        for line in text.lines() {
            let Some(image) = line.trim().strip_prefix("image:") else {
                continue;
            };
            let image = image.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
            let name = image.split(':').next().unwrap_or("");
            let dialect = if name.contains("postgres") || name.contains("postgis") {
                "postgresql"
            } else if name.contains("mysql") || name.contains("mariadb") {
                "mysql"
            } else {
                continue;
            };
            hints.push(DialectHint {
                dialect,
                source: format!("{file}: image {name}"),
            });
            break;
        }
    }
    hints
}

/// Dialects of database driver dependencies (package.json, go.mod, Gemfile,
/// Python requirements, Cargo.toml, mix.exs)
fn dependency_dialects(tree: &Tree) -> Vec<DialectHint> {
    // (manifest file name, dependency text, dialect)
    const DRIVERS: &[(&str, &str, &str)] = &[
        ("package.json", "\"pg\"", "postgresql"),
        ("package.json", "\"postgres\"", "postgresql"),
        ("package.json", "\"slonik\"", "postgresql"),
        ("package.json", "\"pg-promise\"", "postgresql"),
        ("package.json", "\"@neondatabase/serverless\"", "postgresql"),
        ("package.json", "\"@vercel/postgres\"", "postgresql"),
        ("package.json", "\"@supabase/supabase-js\"", "postgresql"),
        ("package.json", "\"mysql2\"", "mysql"),
        ("package.json", "\"mysql\"", "mysql"),
        ("package.json", "\"@planetscale/database\"", "mysql"),
        ("package.json", "\"better-sqlite3\"", "sqlite"),
        ("package.json", "\"sqlite3\"", "sqlite"),
        ("package.json", "\"@libsql/client\"", "sqlite"),
        ("go.mod", "github.com/jackc/pgx", "postgresql"),
        ("go.mod", "github.com/lib/pq", "postgresql"),
        ("go.mod", "github.com/go-sql-driver/mysql", "mysql"),
        ("go.mod", "github.com/mattn/go-sqlite3", "sqlite"),
        ("go.mod", "modernc.org/sqlite", "sqlite"),
        ("Gemfile", "\"pg\"", "postgresql"),
        ("Gemfile", "'pg'", "postgresql"),
        ("Gemfile", "\"mysql2\"", "mysql"),
        ("Gemfile", "'mysql2'", "mysql"),
        ("Gemfile", "\"trilogy\"", "mysql"),
        ("Gemfile", "'trilogy'", "mysql"),
        ("Gemfile", "\"sqlite3\"", "sqlite"),
        ("Gemfile", "'sqlite3'", "sqlite"),
        ("requirements.txt", "psycopg", "postgresql"),
        ("requirements.txt", "asyncpg", "postgresql"),
        ("requirements.txt", "pymysql", "mysql"),
        ("requirements.txt", "mysqlclient", "mysql"),
        ("pyproject.toml", "psycopg", "postgresql"),
        ("pyproject.toml", "asyncpg", "postgresql"),
        ("pyproject.toml", "pymysql", "mysql"),
        ("pyproject.toml", "mysqlclient", "mysql"),
        ("Cargo.toml", "tokio-postgres", "postgresql"),
        ("Cargo.toml", "rusqlite", "sqlite"),
        ("Cargo.toml", "mysql_async", "mysql"),
        ("mix.exs", ":postgrex", "postgresql"),
        ("mix.exs", ":myxql", "mysql"),
        ("mix.exs", ":ecto_sqlite3", "sqlite"),
    ];
    let mut hints: Vec<DialectHint> = Vec::new();
    let manifests: BTreeSet<&str> = DRIVERS.iter().map(|(m, _, _)| *m).collect();
    for manifest in manifests {
        for file in tree
            .named(&[manifest])
            .into_iter()
            .filter(|f| depth(f) <= 2)
        {
            let Some(mut text) = tree.read(file) else {
                continue;
            };
            if manifest == "package.json" {
                text = package_dependencies(&text);
            }
            // sqlx names the database in its features: `features = ["postgres", ...]`
            let sqlx = (manifest == "Cargo.toml")
                .then(|| {
                    text.lines().filter(|l| l.contains("sqlx")).find_map(|l| {
                        [
                            ("postgres", "postgresql"),
                            ("mysql", "mysql"),
                            ("sqlite", "sqlite"),
                        ]
                        .into_iter()
                        .find(|(feature, _)| l.contains(&format!("\"{feature}\"")))
                    })
                })
                .flatten();
            let hint = match sqlx {
                Some((feature, dialect)) => Some((dialect, format!("sqlx \"{feature}\" feature"))),
                None => DRIVERS
                    .iter()
                    .filter(|(m, _, _)| *m == manifest)
                    .find(|(_, dep, _)| text.contains(dep))
                    .map(|(_, dep, dialect)| (*dialect, dep.trim_matches(['"', '\'']).to_string())),
            };
            if let Some((dialect, dep)) = hint {
                hints.push(DialectHint {
                    dialect,
                    source: format!("{file}: {dep}"),
                });
            }
        }
    }
    hints
}

/// The dependency names of a package.json, each quoted on its own line
fn package_dependencies(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return String::new();
    };
    let mut names = String::new();
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = value.get(key).and_then(serde_json::Value::as_object) {
            for name in deps.keys() {
                names.push('"');
                names.push_str(name);
                names.push_str("\"\n");
            }
        }
    }
    names
}

/// SQL text without its `--` and `/* */` comments (which may quote anything,
/// e.g. Prisma's warnings quote column names in backticks)
fn strip_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while !rest.is_empty() {
        let line = rest.find("--");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.map_or(true, |b| l < b) => {
                out.push_str(&rest[..l]);
                rest = rest[l..].find('\n').map_or("", |end| &rest[l + end..]);
            }
            (_, Some(b)) => {
                out.push_str(&rest[..b]);
                rest = rest[b + 2..]
                    .find("*/")
                    .map_or("", |end| &rest[b + 2 + end + 2..]);
            }
            _ => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out
}

/// The dialect suggested by the schema SQL itself
fn sniff_dialect(tree: &Tree, schema: &SchemaSource) -> Option<DialectHint> {
    let mut files: Vec<&String> = schema.schema.iter().collect();
    if let Some(dir) = &schema.schema_dir {
        files.extend(tree.under(dir).filter(|f| is_sql(f)).take(50));
    }
    let (mut pg, mut mysql, mut sqlite) = (0, 0, 0);
    for file in files {
        let Some(text) = tree.read(file) else {
            continue;
        };
        let lower = strip_comments(&text).to_ascii_lowercase();
        pg += [
            "serial",
            "timestamptz",
            "::",
            "create extension",
            "jsonb",
            "bytea",
            "plpgsql",
        ]
        .iter()
        .filter(|m| lower.contains(*m))
        .count();
        mysql += ["auto_increment", "engine=", "engine =", "unsigned", "`"]
            .iter()
            .filter(|m| lower.contains(*m))
            .count();
        sqlite += ["autoincrement", "without rowid", "pragma "]
            .iter()
            .filter(|m| lower.contains(*m))
            .count();
    }
    let (dialect, score) = [("postgresql", pg), ("mysql", mysql), ("sqlite", sqlite)]
        .into_iter()
        .max_by_key(|(_, n)| *n)?;
    let second = [pg, mysql, sqlite]
        .into_iter()
        .filter(|n| *n != score)
        .max()
        .unwrap_or(0);
    (score > 0 && score > second).then(|| DialectHint {
        dialect,
        source: format!("syntax of {}", schema.paths()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_normalizes_paths() {
        assert_eq!(join("", "./sql/schema/"), "sql/schema");
        assert_eq!(join("backend", "../db/schema.sql"), "db/schema.sql");
        assert_eq!(join("backend", "."), "backend");
        assert_eq!(join("a", "b/./c"), "a/b/c");
    }

    #[test]
    fn dump_names() {
        assert!(is_dump("db/schema.sql"));
        assert!(is_dump("sql/app_schema.sql"));
        assert!(!is_dump("cmd/setup/34/34_cache_schema.sql"));
        assert!(!is_dump("cmd/setup/34/cache_schema.sql"));
        assert!(!is_dump("db/users.sql"));
    }

    #[test]
    fn test_directories() {
        assert!(in_test_dir("cli/integration_test/v2/migrations"));
        assert!(in_test_dir("e2e-tests/db/schema.sql"));
        assert!(in_test_dir("pkg/test_data/schema.sql"));
        assert!(in_test_dir("examples/db/schema.sql"));
        assert!(!in_test_dir("db/migrations/0001_contest.sql"));
        assert!(!in_test_dir("server/latest/schema.sql"));
    }

    #[test]
    fn dialect_directories() {
        let source = |dir: &str| SchemaSource {
            label: String::new(),
            schema: Vec::new(),
            schema_dir: Some(dir.to_string()),
            sql_files: 1,
        };
        assert_eq!(
            path_dialect(&source("db/migrations/postgres")),
            Some("postgresql")
        );
        assert_eq!(path_dialect(&source("db/h2")), Some("other"));
        assert_eq!(path_dialect(&source("db/migrations")), None);
    }

    #[test]
    fn strips_comments() {
        assert_eq!(
            strip_comments("-- drops `a`\nSELECT 1; /* `b` */ SELECT 2;"),
            "\nSELECT 1;  SELECT 2;"
        );
        assert_eq!(strip_comments("SELECT 1 /* open"), "SELECT 1 ");
    }

    #[test]
    fn flyway_names() {
        assert!(is_flyway("V1__init.sql"));
        assert!(is_flyway("V2_1__add.sql"));
        assert!(is_flyway("R__views.sql"));
        assert!(!is_flyway("U1__undo.sql"));
        assert!(!is_flyway("Vx__init.sql"));
        assert!(!is_flyway("init.sql"));
    }

    #[test]
    fn finds_template_tags() {
        assert!(template_tags("const r = await sql`SELECT 1`").sql);
        assert!(template_tags("db.sql`SELECT 1`").sql);
        assert!(template_tags("sql<User[]>`SELECT * FROM users`").sql);
        assert!(template_tags("sql.type(User)`SELECT 1`").sql);
        assert!(!template_tags("import mysql from 'mysql2'; mysql`x`").sql);
        assert!(!template_tags("const sql = 'SELECT 1'; if (a<b) {}").sql);
        let prisma = template_tags("await prisma.$queryRaw<User[]>`SELECT 1`");
        assert!(prisma.prisma_raw && !prisma.sql);
    }

    #[test]
    fn finds_sqlx_queries() {
        assert!(uses_sqlx_queries("sqlx::query!(\"SELECT 1\")"));
        assert!(uses_sqlx_queries("query_as! (User, \"SELECT 1\")"));
        assert!(uses_sqlx_queries(
            "sqlx::query_scalar::<_, i64>(\"SELECT 1\")"
        ));
        assert!(uses_sqlx_queries(
            "sqlx::query_as_unchecked!(User, \"SELECT 1\")"
        ));
        assert!(!uses_sqlx_queries("sqlx::query_file!(\"q.sql\")"));
        assert!(!uses_sqlx_queries(
            "let q = query(\"SELECT 1\"); my_query!(x)"
        ));
    }

    #[test]
    fn finds_python_queries() {
        assert!(uses_python_queries("cur.execute(\"SELECT 1\")"));
        assert!(uses_python_queries(
            "await conn.fetch('''\n  SELECT 1\n''', x)"
        ));
        assert!(!uses_python_queries("cur.execute(query)"));
        assert!(!uses_python_queries("subprocess.run(\"SELECT\")"));
    }

    #[test]
    fn reads_config_strings() {
        let prisma = "generator client {\n  provider = \"prisma-client-js\"\n}\n\ndatasource db {\n  provider = \"postgresql\"\n  url = env(\"DATABASE_URL\")\n}\n";
        assert_eq!(prisma_provider(prisma).as_deref(), Some("postgresql"));
        let drizzle =
            "export default defineConfig({\n  out: './drizzle',\n  dialect: 'mysql',\n});";
        assert_eq!(drizzle_dialect(drizzle), Some("mysql"));
        assert_eq!(config_string(drizzle, "out").as_deref(), Some("./drizzle"));
    }

    #[test]
    fn reads_yaml_lists() {
        assert_eq!(
            yaml_list(
                "name: x\nmodel-paths: [\"models\", 'more']\n",
                "model-paths"
            ),
            Some(vec!["models".to_string(), "more".to_string()])
        );
        assert_eq!(
            yaml_list(
                "model-paths:\n  - models\n  - other\nseed-paths: []",
                "model-paths"
            ),
            Some(vec!["models".to_string(), "other".to_string()])
        );
        assert_eq!(yaml_list("name: x\n", "model-paths"), None);
    }

    #[test]
    fn url_schemes() {
        assert_eq!(url_dialect("postgres://u:p@h/db"), Some("postgresql"));
        assert_eq!(url_dialect("\"mysql://h/db\""), Some("mysql"));
        assert_eq!(url_dialect("file:./dev.db"), Some("sqlite"));
        assert_eq!(url_dialect("https://x"), None);
    }

    #[test]
    fn groups_query_files_around_schema_sources() {
        let set = |items: &[&str]| -> BTreeSet<String> {
            items.iter().map(ToString::to_string).collect()
        };
        let files = set(&[
            "db/queries/users.sql",
            "db/queries/posts/list.sql",
            "src/a/q.sql",
            "seed.sql",
            "db/seed.sql",
        ]);
        let excluded = set(&["db/migrations/001.sql", "db/schema.sql"]);
        let all: Vec<String> = files.iter().chain(&excluded).cloned().collect();
        let all: Vec<&String> = all.iter().collect();
        let (groups, ignore) = group_files(&files, &excluded, &all);
        let patterns: Vec<&str> = groups.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            patterns,
            [
                "db/queries/**/*.sql",
                "db/seed.sql",
                "seed.sql",
                "src/**/*.sql"
            ]
        );
        assert!(ignore.is_empty());
    }
}
