//! Configuration file handling

use miette::{IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sqlsift_core::rules::{
    find_category, find_rule, similar_category_name, similar_rule_name, RuleConfig, RuleLevel,
};
use sqlsift_core::Templating;

/// Keys recognized in `sqlsift.toml`
const KNOWN_KEYS: &[&str] = &[
    "schema",
    "files",
    "ignore",
    "dialect",
    "format",
    "disable",
    "schema_dir",
    "rules",
    "categories",
    "max_warnings",
    "templating",
    "baseline",
    "embedded_sql_tags",
];

/// Configuration for sqlsift
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Schema file paths or patterns
    #[serde(default)]
    pub schema: Vec<String>,

    /// Query file patterns to check
    #[serde(default)]
    pub files: Vec<String>,

    /// Glob patterns of query files to skip (applied to `files` and to files
    /// given on the command line)
    #[serde(default)]
    pub ignore: Vec<String>,

    /// SQL dialect ("postgresql", "mysql", "sqlite")
    #[serde(default)]
    pub dialect: Option<String>,

    /// Query file templating ("jinja", "none"); when unset, Jinja is used if a
    /// `dbt_project.yml` is next to the config file, in the current directory or in
    /// an ancestor directory of the query file
    #[serde(default)]
    pub templating: Option<String>,

    /// Output format (human, json, sarif, github)
    #[serde(default)]
    pub format: Option<String>,

    /// Fail the check when more than this many warnings are reported
    #[serde(default)]
    pub max_warnings: Option<usize>,

    /// Baseline file of known diagnostics that are not reported
    #[serde(default)]
    pub baseline: Option<String>,

    /// Tags of the template literals checked as SQL in TypeScript and JavaScript
    /// files (default `["sql"]`); matched against the last identifier of the tag,
    /// so `"sql"` also matches `db.sql` and `"$queryRaw"` matches `prisma.$queryRaw`
    #[serde(default)]
    pub embedded_sql_tags: Option<Vec<String>>,

    /// Rules to disable (e.g., `["E0001", "E0002"]`)
    #[serde(default)]
    pub disable: Vec<String>,

    /// Rule levels by rule code or name (e.g. `E0006 = "warn"`, `column-not-found = "off"`)
    #[serde(default)]
    pub rules: BTreeMap<String, String>,

    /// Levels of whole rule categories (e.g. `suspicious = "error"`)
    #[serde(default)]
    pub categories: BTreeMap<String, String>,

    /// Schema directory
    pub schema_dir: Option<String>,

    /// Directory of the configuration file this was loaded from
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
}

impl Config {
    /// Load configuration from a TOML file.
    ///
    /// Relative paths in `schema`, `files`, `ignore`, `schema_dir` and `baseline` are
    /// resolved against the directory containing the configuration file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| miette::miette!("Failed to read config file {}: {}", path.display(), e))?;
        let table: toml::Table = toml::from_str(&contents)
            .map_err(|e| miette::miette!("Failed to parse {}: {}", path.display(), e))?;
        for key in table.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                eprintln!(
                    "Warning: {}: unknown key '{}' (known keys: {})",
                    path.display(),
                    key,
                    KNOWN_KEYS.join(", ")
                );
            }
        }
        let mut config: Config = table
            .try_into()
            .map_err(|e| miette::miette!("Failed to parse {}: {}", path.display(), e))?;

        let base = config_base_dir(path);
        config.base_dir = Some(base.clone());
        let resolve = |p: &String| resolve_path(&base, p);
        config.schema = config.schema.iter().map(resolve).collect();
        config.files = config.files.iter().map(resolve).collect();
        config.schema_dir = config.schema_dir.as_ref().map(resolve);
        config.baseline = config.baseline.as_ref().map(resolve);
        // Ignore patterns are matched later; the base directory is literal text
        for pattern in &config.ignore {
            glob::Pattern::new(pattern).map_err(|e| {
                miette::miette!(
                    "{}: invalid ignore pattern '{}': {}",
                    path.display(),
                    pattern,
                    e
                )
            })?;
        }
        let escaped_base = PathBuf::from(glob::Pattern::escape(&base.to_string_lossy()));
        config.ignore = config
            .ignore
            .iter()
            .map(|p| resolve_path(&escaped_base, p))
            .collect();
        Ok(config)
    }

    /// Try to find and load sqlsift.toml in current directory or parent directories
    pub fn find_and_load() -> Result<Option<Self>> {
        let mut current_dir = std::env::current_dir().into_diagnostic()?;

        loop {
            let config_path = current_dir.join("sqlsift.toml");
            if config_path.exists() {
                return Ok(Some(Self::from_file(&config_path)?));
            }

            // Try parent directory
            if !current_dir.pop() {
                break;
            }
        }

        Ok(None)
    }

    /// Merge CLI arguments into configuration
    /// CLI arguments take precedence over config file values; `--ignore`
    /// patterns are added to the config file's `ignore`
    #[allow(clippy::too_many_arguments)]
    pub fn merge_with_args(
        mut self,
        schema: &[PathBuf],
        schema_dir: Option<&Path>,
        files: &[PathBuf],
        ignore: &[String],
        format: Option<crate::args::OutputFormat>,
        dialect: Option<&str>,
        templating: Option<&str>,
        max_warnings: Option<usize>,
        baseline: Option<&Path>,
    ) -> Self {
        self.ignore.extend(ignore.iter().cloned());

        // CLI args override config file
        if !schema.is_empty() {
            self.schema = schema.iter().map(|p| p.display().to_string()).collect();
        }

        if let Some(dir) = schema_dir {
            self.schema_dir = Some(dir.display().to_string());
        }

        if !files.is_empty() {
            self.files = files.iter().map(|p| p.display().to_string()).collect();
        }

        if let Some(fmt) = format {
            self.format = Some(format!("{fmt:?}").to_lowercase());
        }

        if let Some(dialect) = dialect {
            self.dialect = Some(dialect.to_string());
        }

        if let Some(templating) = templating {
            self.templating = Some(templating.to_string());
        }

        if max_warnings.is_some() {
            self.max_warnings = max_warnings;
        }

        if let Some(baseline) = baseline {
            self.baseline = Some(baseline.display().to_string());
        }

        self
    }
}

impl Config {
    /// The configured templating of query files. When unset, Jinja is used for dbt
    /// projects: a `dbt_project.yml` in the current directory or in the directory
    /// of the configuration file (query files in a directory with a
    /// `dbt_project.yml` above them are Jinja templates too).
    pub fn templating(&self) -> Result<Templating> {
        if let Some(templating) = &self.templating {
            return templating.parse().map_err(|e: String| miette::miette!(e));
        }
        let dirs = [Some(Path::new("")), self.base_dir.as_deref()];
        let dbt = dirs
            .into_iter()
            .flatten()
            .any(|dir| dir.join("dbt_project.yml").is_file());
        Ok(if dbt {
            Templating::Jinja
        } else {
            Templating::None
        })
    }
}

/// Rule levels set on the command line (`-A`, `-W`, `-D`)
#[derive(Debug, Default)]
pub struct RuleFlags<'a> {
    pub allow: &'a [String],
    pub warn: &'a [String],
    pub deny: &'a [String],
}

impl Config {
    /// Template literal tags whose SQL is checked in TypeScript and JavaScript files
    pub fn embedded_sql_tags(&self) -> Vec<String> {
        self.embedded_sql_tags.clone().unwrap_or_else(|| {
            sqlsift_core::embedded::DEFAULT_TAGS
                .iter()
                .map(ToString::to_string)
                .collect()
        })
    }

    /// Rule levels from the config file, overridden by command line flags.
    /// A rule's own level always takes precedence over its category's.
    pub fn rule_config(&self, flags: &RuleFlags) -> Result<RuleConfig> {
        let mut rules = RuleConfig::default();
        let mut set = |id: &str, level: RuleLevel, origin: &str| {
            rules
                .configure(id, level)
                .map_err(|e| miette::miette!("{}: {}", origin, e))
        };

        for (name, level) in &self.categories {
            if find_category(name).is_none() {
                let hint = match similar_category_name(name) {
                    Some(suggestion) => format!(". Did you mean '{suggestion}'?"),
                    None => String::new(),
                };
                miette::bail!(
                    "[categories]: unknown category '{}'{} (expected one of: {})",
                    name,
                    hint,
                    sqlsift_core::RuleCategory::ALL.map(|c| c.name()).join(", ")
                );
            }
            set(name, parse_level(level, "[categories]")?, "[categories]")?;
        }
        for (id, level) in &self.rules {
            if find_rule(id).is_none() {
                let hint = match similar_rule_name(id) {
                    Some(suggestion) => format!(". Did you mean '{suggestion}'?"),
                    None => String::new(),
                };
                miette::bail!(
                    "[rules]: unknown rule '{}'{} (run `sqlsift rules` to list rules)",
                    id,
                    hint
                );
            }
            set(id, parse_level(level, "[rules]")?, "[rules]")?;
        }
        for id in &self.disable {
            set(id, RuleLevel::Off, "disable")?;
        }
        for (ids, level, flag) in [
            (flags.allow, RuleLevel::Off, "--allow"),
            (flags.warn, RuleLevel::Warn, "--warn"),
            (flags.deny, RuleLevel::Error, "--deny"),
        ] {
            for id in ids {
                set(id, level, flag)?;
            }
        }
        Ok(rules)
    }
}

/// Parse a rule level from the config file
fn parse_level(level: &str, origin: &str) -> Result<RuleLevel> {
    level
        .parse()
        .map_err(|e: String| miette::miette!("{}: {}", origin, e))
}

/// Directory that relative paths in a config file are resolved against.
///
/// When the config lives under the current directory the result is kept
/// relative, so that reported file names stay short (and SARIF URIs stay
/// relative to the project root).
fn config_base_dir(config_path: &Path) -> PathBuf {
    let parent = config_path.parent().unwrap_or(Path::new("")).to_path_buf();
    if parent.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(rel) = parent.strip_prefix(&cwd) {
                return rel.to_path_buf();
            }
        }
    }
    parent
}

fn resolve_path(base: &Path, path: &str) -> String {
    if base.as_os_str().is_empty() || Path::new(path).is_absolute() {
        path.to_string()
    } else {
        base.join(path).display().to_string()
    }
}
