//! CLI argument definitions

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "sqlsift")]
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

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// Turn rules or rule categories off (e.g. `-A E0006`, `-A ambiguous-column`)
        #[arg(
            short = 'A',
            long = "allow",
            visible_alias = "disable",
            value_name = "RULE"
        )]
        allow: Vec<String>,

        /// Report rules or rule categories as warnings, which don't fail the check
        #[arg(short = 'W', long = "warn", value_name = "RULE")]
        warn: Vec<String>,

        /// Report rules or rule categories as errors (e.g. `-D suspicious`)
        #[arg(short = 'D', long = "deny", value_name = "RULE")]
        deny: Vec<String>,

        /// SQL dialect: postgresql, mysql, sqlite [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Output format
        #[arg(short, long, value_enum)]
        format: Option<OutputFormat>,

        /// Maximum number of errors before stopping
        #[arg(long, default_value = "100")]
        max_errors: usize,
    },

    /// List all rules with their category and default level
    Rules,

    /// Display the schema sqlsift loaded (tables, views, enum types)
    ///
    /// Schema files are taken from the arguments, or from sqlsift.toml when none are given.
    Schema {
        /// Schema definition files (same as --schema; supports glob patterns)
        files: Vec<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// SQL dialect: postgresql, mysql, sqlite [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = SchemaFormat::Human)]
        format: SchemaFormat,
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

/// Output format of `sqlsift schema`
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Default)]
pub enum SchemaFormat {
    /// Human-readable listing
    #[default]
    Human,
    /// JSON output
    Json,
}
