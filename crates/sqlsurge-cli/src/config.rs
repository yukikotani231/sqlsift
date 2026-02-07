//! Configuration file handling

use miette::{IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for sqlsurge
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Schema file paths or patterns
    #[serde(default)]
    pub schema: Vec<String>,

    /// Query file patterns to check
    #[serde(default)]
    pub files: Vec<String>,

    /// Output format (human, json, sarif)
    #[serde(default)]
    pub format: Option<String>,

    /// Rules to disable (e.g., ["E0001", "E0002"])
    #[serde(default)]
    pub disable: Vec<String>,

    /// Schema directory
    pub schema_dir: Option<String>,
}

impl Config {
    /// Load configuration from a TOML file
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(path).into_diagnostic()?;
        let config: Config = toml::from_str(&contents).into_diagnostic()?;
        Ok(config)
    }

    /// Try to find and load sqlsurge.toml in current directory or parent directories
    pub fn find_and_load() -> Result<Option<Self>> {
        let mut current_dir = std::env::current_dir().into_diagnostic()?;

        loop {
            let config_path = current_dir.join("sqlsurge.toml");
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
    /// CLI arguments take precedence over config file values
    pub fn merge_with_args(
        mut self,
        schema: &[PathBuf],
        schema_dir: &Option<PathBuf>,
        files: &[PathBuf],
        format: &Option<crate::args::OutputFormat>,
        disable: &[String],
    ) -> Self {
        // CLI args override config file
        if !schema.is_empty() {
            self.schema = schema.iter().map(|p| p.display().to_string()).collect();
        }

        if schema_dir.is_some() {
            self.schema_dir = schema_dir.as_ref().map(|p| p.display().to_string());
        }

        if !files.is_empty() {
            self.files = files.iter().map(|p| p.display().to_string()).collect();
        }

        if let Some(fmt) = format {
            self.format = Some(format!("{:?}", fmt).to_lowercase());
        }

        if !disable.is_empty() {
            self.disable = disable.to_vec();
        }

        self
    }
}
