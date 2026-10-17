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

        /// With --write-baseline: only rewrite the entries of these rules (codes or
        /// names, repeatable), keeping the other entries of the checked files as
        /// they are (e.g. after turning a rule on)
        #[arg(
            long = "baseline-rule",
            value_name = "RULE",
            requires = "write_baseline"
        )]
        baseline_rule: Vec<String>,

        /// Remove the baseline entries that no longer occur (problems fixed, files
        /// deleted or ignored) and exit 0; never adds entries
        #[arg(long, conflicts_with = "write_baseline")]
        prune_baseline: bool,

        /// Fail (exit 1) when baseline entries no longer occur, so that the baseline
        /// only shrinks (remove them with --prune-baseline)
        #[arg(long)]
        strict_baseline: bool,

        /// Fail (exit 1) when the baseline has more than N entries, so that a change
        /// can't grow it
        #[arg(long, value_name = "N")]
        max_baseline_entries: Option<usize>,
    },

    /// Show the queries a schema change breaks or fixes
    ///
    /// Builds the schema without and with the given migrations, checks the query
    /// files against both and reports the diagnostics that differ. Migrations that
    /// are among the schema files are left out of the "before" schema together
    /// with the schema files after them; others are applied after all schema
    /// files. Only query files that name a changed table or view are checked.
    /// Exits 1 when the change introduces errors.
    Impact {
        /// Migration (schema) files whose effect is shown (supports glob patterns)
        #[arg(required_unless_present = "base")]
        migrations: Vec<PathBuf>,

        /// Show the effect of the schema files added or changed since the commit
        /// where the current branch left this git revision (e.g. `origin/main`),
        /// uncommitted and untracked files included
        #[arg(long, value_name = "REV")]
        base: Option<String>,

        /// Also list the queries that use a changed table or view without a
        /// diagnostic changing (where to look when reviewing the change)
        #[arg(long)]
        references: bool,

        /// Query files to check (supports glob patterns) [default: `files` in sqlsift.toml]
        #[arg(long = "queries", value_name = "PATTERN")]
        queries: Vec<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Skip query files matching a glob pattern (repeatable)
        #[arg(long = "ignore", value_name = "PATTERN")]
        ignore: Vec<String>,

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// Turn rules or rule categories off
        #[arg(
            short = 'A',
            long = "allow",
            visible_alias = "disable",
            value_name = "RULE"
        )]
        allow: Vec<String>,

        /// Report rules or rule categories as warnings
        #[arg(short = 'W', long = "warn", value_name = "RULE")]
        warn: Vec<String>,

        /// Report rules or rule categories as errors
        #[arg(short = 'D', long = "deny", value_name = "RULE")]
        deny: Vec<String>,

        /// SQL dialect [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Query file templating: jinja, none
        #[arg(long, value_name = "ENGINE")]
        templating: Option<String>,

        /// dbt catalog.json with the columns of models and sources (alpha)
        #[arg(long = "dbt-catalog", value_name = "PATH")]
        dbt_catalog: Option<PathBuf>,

        /// Output format (SARIF and GitHub annotations show what the change breaks)
        #[arg(short, long, value_enum)]
        format: Option<OutputFormat>,
    },

    /// List the tables and columns that no query uses
    ///
    /// Checks every query file (and the schema's views) and lists the tables they
    /// never reference, then the columns of used tables they never read: never
    /// referenced, or only written (INSERT, UPDATE SET). `SELECT *` reads every
    /// column; columns of keys, foreign keys and CHECK constraints count as used.
    /// Columns may be used outside the checked queries, so this is a list of
    /// candidates to look at, not of columns to drop. Exits 0 unless
    /// --fail-on-unused.
    Unused {
        /// Query files to check (supports glob patterns) [default: `files` in sqlsift.toml]
        files: Vec<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Skip query files matching a glob pattern (repeatable)
        #[arg(long = "ignore", value_name = "PATTERN")]
        ignore: Vec<String>,

        /// Never report tables or columns matching a glob pattern of `table`,
        /// `schema.table`, `table.column` or `schema.table.column` (repeatable)
        #[arg(long = "exclude", value_name = "PATTERN")]
        exclude: Vec<String>,

        /// Exit 1 when anything is unused
        #[arg(long)]
        fail_on_unused: bool,

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// SQL dialect [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Query file templating: jinja, none
        #[arg(long, value_name = "ENGINE")]
        templating: Option<String>,

        /// dbt catalog.json with the columns of models and sources (alpha)
        #[arg(long = "dbt-catalog", value_name = "PATH")]
        dbt_catalog: Option<PathBuf>,

        /// Output format: human or json
        #[arg(short, long, value_enum)]
        format: Option<OutputFormat>,
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
    Rules {
        /// Output format
        #[arg(short, long, value_enum, default_value_t = RulesFormat::Human)]
        format: RulesFormat,
    },

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

    /// Describe what each query returns: its output columns and their types
    ///
    /// Queries are found as `check` finds them (.sql files, sqlc / aiosql / HugSQL
    /// names, SQL in TypeScript / JavaScript tagged templates, dbt models). A name
    /// or type sqlsift can't tell is null, never guessed.
    Describe {
        /// SQL, TypeScript or JavaScript files to describe (supports glob patterns)
        files: Vec<PathBuf>,

        /// Schema definition files
        #[arg(short, long = "schema", value_name = "FILE")]
        schema: Vec<PathBuf>,

        /// Directory containing schema files
        #[arg(long = "schema-dir", value_name = "DIR")]
        schema_dir: Option<PathBuf>,

        /// Skip query files matching a glob pattern (repeatable)
        #[arg(long = "ignore", value_name = "PATTERN")]
        ignore: Vec<String>,

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks [default: postgresql]
        #[arg(short, long)]
        dialect: Option<String>,

        /// Query file templating: jinja (dbt models), none [default: as for `check`]
        #[arg(long, value_name = "ENGINE")]
        templating: Option<String>,

        /// dbt catalog.json with the columns of the models and sources (alpha)
        #[arg(long = "dbt-catalog", value_name = "PATH")]
        dbt_catalog: Option<PathBuf>,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = DescribeFormat::Human)]
        format: DescribeFormat,
    },

    /// Inspect the baseline file of known diagnostics
    Baseline {
        #[command(subcommand)]
        command: BaselineCommand,
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
    /// GitLab Code Quality report
    Gitlab,
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

/// Output format of `sqlsift describe`
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Default)]
pub enum DescribeFormat {
    /// Human-readable listing
    #[default]
    Human,
    /// JSON output
    Json,
}

/// Subcommands of `sqlsift baseline`
#[derive(Subcommand)]
pub enum BaselineCommand {
    /// Count the baseline entries by rule and by file
    Stats {
        /// Baseline file [default: `baseline` in sqlsift.toml, or sqlsift-baseline.json]
        #[arg(long, value_name = "PATH")]
        baseline: Option<PathBuf>,

        /// Path to configuration file (default: sqlsift.toml in current or parent directory)
        #[arg(short, long = "config", value_name = "FILE")]
        config: Option<PathBuf>,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = BaselineFormat::Human)]
        format: BaselineFormat,
    },
}

/// Output format of `sqlsift baseline stats`
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Default)]
pub enum BaselineFormat {
    /// Human-readable listing
    #[default]
    Human,
    /// JSON output
    Json,
}

/// Output format of `sqlsift rules`
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum, Default)]
pub enum RulesFormat {
    /// Human-readable table
    #[default]
    Human,
    /// JSON output
    Json,
}
