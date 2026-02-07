//! CLI argument definitions

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "sqlsurge")]
#[command(author, version, about = "SQL static analysis tool")]
#[command(propagate_version = true)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,

    /// Enable verbose output
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check SQL files against schema definitions
    Check {
        /// SQL files to check (supports glob patterns)
        files: Vec<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Path to configuration file (default: sqlsurge.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// Disable specific rules (e.g., E0001, E0002)
        #[arg(long = "disable", value_name = "RULE")]
        disable: Vec<String>,

        /// SQL dialect
        #[arg(short, long, default_value = "postgresql")]
        dialect: String,

        /// Output format
        #[arg(short, long, value_enum)]
        format: Option<OutputFormat>,

        /// Maximum number of errors before stopping
        #[arg(long, default_value = "100")]
        max_errors: usize,
    },

    /// Display schema information
    Schema {
        /// Schema definition files
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Parse SQL and display AST (for debugging)
    Parse {
        /// SQL file to parse
        file: PathBuf,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputFormat {
    /// Human-readable output with colors
    #[default]
    Human,
    /// JSON output
    Json,
    /// SARIF output (for GitHub Code Scanning)
    Sarif,
}
