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

// Parsed once per run, so the size difference between variants doesn't matter
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// Check SQL files against schema definitions
    Check {
        /// SQL, TypeScript or JavaScript files to check (supports glob patterns; `-` reads a query from stdin)
        files: Vec<PathBuf>,

        /// File name to report for the query read from stdin (`-`)
        #[arg(long = "stdin-filename", value_name = "PATH")]
        stdin_filename: Option<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Skip query files matching a glob pattern (repeatable, e.g. `--ignore 'sql/archive/**'`)
        #[arg(long = "ignore", value_name = "PATTERN")]
        ignore: Vec<String>,

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

        /// SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Query file templating: jinja (dbt models), none [default: jinja when
        /// dbt_project.yml is in the current or the config file's directory or above
        /// the query file, else none]
        #[arg(long, value_name = "ENGINE")]
        templating: Option<String>,

        /// dbt catalog.json (from `dbt docs generate`) with the columns of the models
        /// and sources that `ref()` / `source()` name (alpha) [default:
        /// target/catalog.json of the dbt project, if it exists]
        #[arg(long = "dbt-catalog", value_name = "PATH")]
        dbt_catalog: Option<PathBuf>,

        /// Output format
        #[arg(short, long, value_enum)]
        format: Option<OutputFormat>,

        /// Maximum number of errors before stopping
        #[arg(long, default_value = "100")]
        max_errors: usize,

        /// Fail (exit 1) when more than N warnings are reported
        #[arg(long, value_name = "N")]
        max_warnings: Option<usize>,

        /// Baseline file of known diagnostics, which are not reported
        /// (default: `baseline` in sqlsift.toml)
        #[arg(long, value_name = "PATH")]
        baseline: Option<PathBuf>,

        /// Write every current diagnostic of the checked files to the baseline file and
        /// exit 0, keeping the entries of other files that still exist (the file is
        /// `--baseline`, `baseline` in sqlsift.toml, or sqlsift-baseline.json)
        #[arg(long)]
        write_baseline: bool,
    },

    /// Detect the project layout and write sqlsift.toml
    ///
    /// Finds the schema (sqlc configuration, Prisma / Supabase / Drizzle / Flyway /
    /// golang-migrate / goose / dbmate migrations, schema dumps, dbt catalogs), the
    /// query files (.sql files, sqlc queries, Prisma TypedSQL, dbt models, SQL in
    /// TypeScript / JavaScript tagged templates) and the dialect, writes a commented
    /// sqlsift.toml, runs a first check and prints a summary and the next steps.
    /// Never asks questions.
    Init {
        /// Project directory [default: current directory]
        dir: Option<PathBuf>,

        /// Overwrite an existing sqlsift.toml
        #[arg(long)]
        force: bool,

        /// SQL dialect to write instead of the detected one
        #[arg(short, long)]
        dialect: Option<String>,

        /// Don't run a first check after writing sqlsift.toml
        #[arg(long)]
        no_check: bool,

        /// Print the configuration to stdout instead of writing sqlsift.toml
        #[arg(long)]
        dry_run: bool,
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

        /// SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks [default: postgresql]
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
    /// GitHub Actions workflow commands (annotations on pull requests)
    Github,
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
