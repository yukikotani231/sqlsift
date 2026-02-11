use std::path::Path;

use serde::{Deserialize, Serialize};

/// Configuration for sqlsurge (loaded from sqlsurge.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub schema: Vec<String>,

    #[serde(default)]
    pub files: Vec<String>,

    #[serde(default)]
    pub dialect: Option<String>,

    #[serde(default)]
    pub format: Option<String>,

    #[serde(default)]
    pub disable: Vec<String>,

    pub schema_dir: Option<String>,
}

impl Config {
    /// Find and load sqlsurge.toml from the given root directory or its parents
    pub fn find_from_root(root: &Path) -> Option<Self> {
        let mut current = root.to_path_buf();
        loop {
            let config_path = current.join("sqlsurge.toml");
            if config_path.exists() {
                let contents = std::fs::read_to_string(&config_path).ok()?;
                return toml::from_str(&contents).ok();
            }
            if !current.pop() {
                break;
            }
        }
        None
    }
}
