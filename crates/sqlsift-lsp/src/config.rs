use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlsift_core::rules::{RuleConfig, RuleLevel};

/// Configuration for sqlsift (loaded from sqlsift.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub schema: Vec<String>,

    #[serde(default)]
    pub files: Vec<String>,

    /// Glob patterns (relative to the config file's directory) of files that
    /// get no diagnostics
    #[serde(default)]
    pub ignore: Vec<String>,

    #[serde(default)]
    pub dialect: Option<String>,

    #[serde(default)]
    pub format: Option<String>,

    #[serde(default)]
    pub disable: Vec<String>,

    /// Rule levels by rule code or name (e.g. `E0006 = "warn"`)
    #[serde(default)]
    pub rules: BTreeMap<String, String>,

    /// Levels of whole rule categories (e.g. `suspicious = "error"`)
    #[serde(default)]
    pub categories: BTreeMap<String, String>,

    pub schema_dir: Option<String>,
}

impl Config {
    /// Rule levels from `[categories]`, `[rules]` and `disable`, with a message for
    /// each setting that names no rule or category or has an invalid level
    pub fn rule_config(&self) -> (RuleConfig, Vec<String>) {
        let mut rules = RuleConfig::default();
        let mut problems = Vec::new();
        let settings = self
            .categories
            .iter()
            .map(|(id, level)| ("categories", id, level.parse::<RuleLevel>()))
            .chain(
                self.rules
                    .iter()
                    .map(|(id, level)| ("rules", id, level.parse::<RuleLevel>())),
            )
            .chain(
                self.disable
                    .iter()
                    .map(|id| ("disable", id, Ok(RuleLevel::Off))),
            );
        for (origin, id, level) in settings {
            let result =
                level.and_then(|level| rules.configure(id, level).map_err(|e| e.to_string()));
            if let Err(e) = result {
                problems.push(format!("{origin}: {e}"));
            }
        }
        (rules, problems)
    }

    /// Find sqlsift.toml in the given root directory or its parents.
    ///
    /// Returns the path of the config file that was found together with the
    /// parsed configuration, or a human-readable error if it could not be loaded.
    pub fn find_from_root(root: &Path) -> Option<(PathBuf, Result<Self, String>)> {
        let mut current = root.to_path_buf();
        loop {
            let config_path = current.join("sqlsift.toml");
            if config_path.exists() {
                let result = std::fs::read_to_string(&config_path)
                    .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))
                    .and_then(|contents| {
                        toml::from_str(&contents).map_err(|e| {
                            format!("Failed to parse {}: {}", config_path.display(), e)
                        })
                    });
                return Some((config_path, result));
            }
            if !current.pop() {
                break;
            }
        }
        None
    }
}
