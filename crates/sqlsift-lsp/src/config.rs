use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Configuration for sqlsift (loaded from sqlsift.toml)
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
