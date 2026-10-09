//! sqlsift CLI - SQL static analysis tool

mod args;
mod config;
mod output;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use miette::Result;
use sqlsift_core::ignore::IgnorePatterns;
use sqlsift_core::schema::{is_rollback_migration, Catalog, SchemaBuilder};
use sqlsift_core::{Analyzer, Diagnostic, RuleConfig, SqlDialect};

use crate::args::{Args, Command, OutputFormat, SchemaFormat};
use crate::config::{Config, RuleFlags};
use crate::output::schema::SchemaReport;
use crate::output::{FileDiagnostics, OutputFormatter};

fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(args.verbose, args.quiet);

    match run(args) {
        Ok(has_errors) => {
            if has_errors {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("Error: {e:?}");
            ExitCode::from(2)
        }
    }
}

fn init_tracing(verbose: u8, quiet: bool) {
    let level = if quiet {
        tracing::Level::ERROR
    } else {
        match verbose {
            0 => tracing::Level::WARN,
            1 => tracing::Level::INFO,
            _ => tracing::Level::DEBUG,
        }
    };

    // Logs go to stderr so that JSON/SARIF on stdout stay machine-readable
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .with_ansi(output::use_color())
        .init();
}

/// A query file's contents and its diagnostics
type AnalyzedFile = Result<(String, Vec<Diagnostic>)>;

/// The file argument that reads a query from stdin
const STDIN_ARG: &str = "-";

/// Name reported for the query read from stdin without `--stdin-filename`
const STDIN_DEFAULT_NAME: &str = "<stdin>";

/// Whether a query file argument means stdin
fn is_stdin(path: &Path) -> bool {
    path.as_os_str() == STDIN_ARG
}

/// Read and analyze each query file, in parallel across the available cores.
/// Results are returned in the same order as `files`. The file `-` is the
/// query read from stdin, `stdin`.
fn analyze_files(
    files: &[PathBuf],
    stdin: Option<&str>,
    catalog: &Catalog,
    dialect: SqlDialect,
    rules: &RuleConfig,
) -> Vec<AnalyzedFile> {
    let analyze_one = |path: &PathBuf| -> AnalyzedFile {
        tracing::debug!(file = %path.display(), "Analyzing SQL file");
        let content = match stdin {
            Some(stdin) if is_stdin(path) => stdin.to_string(),
            _ => read_file(path)?,
        };
        let diagnostics = Analyzer::with_dialect(catalog, dialect)
            .with_rules(rules.clone())
            .analyze(&content);
        Ok((content, diagnostics))
    };

    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(files.len());
    if workers <= 1 {
        return files.iter().map(analyze_one).collect();
    }

    // Workers take the next unclaimed file until none are left
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<Option<AnalyzedFile>> = (0..files.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(path) = files.get(i) else {
                            return done;
                        };
                        done.push((i, analyze_one(path)));
                    }
                })
            })
            .collect();
        for handle in handles {
            for (i, result) in handle.join().expect("analysis thread panicked") {
                results[i] = Some(result);
            }
        }
    });
    results
        .into_iter()
        .map(|r| r.expect("every file is analyzed"))
        .collect()
}

/// Print the rule registry as a table
fn print_rules() {
    use sqlsift_core::rules::RULES;
    let name_width = RULES.iter().map(|r| r.name.len()).max().unwrap_or(0);
    println!(
        "{:<6} {:<name_width$} {:<12} {:<8} DESCRIPTION",
        "CODE", "NAME", "CATEGORY", "DEFAULT"
    );
    for rule in RULES {
        println!(
            "{:<6} {:<name_width$} {:<12} {:<8} {}",
            rule.code,
            rule.name,
            rule.category.name(),
            rule.default_level().name(),
            rule.summary
        );
    }
}

/// Read a file, naming the path in the error message
fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|e| miette::miette!("Failed to read {}: {}", path.display(), e))
}

/// Whether a path pattern contains glob metacharacters
fn is_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// Expand a glob pattern into matching paths (sorted, as returned by `glob`)
fn expand_glob(pattern: &str) -> Result<Vec<PathBuf>> {
    let paths = glob::glob(pattern)
        .map_err(|e| miette::miette!("Invalid glob pattern '{}': {}", pattern, e))?;
    Ok(paths.flatten().collect())
}

/// Load the configuration file given with `--config`, or discover `sqlsift.toml`
/// in the current or a parent directory (an empty configuration when there is none)
fn load_config(config_path: Option<&Path>) -> Result<Config> {
    match config_path {
        Some(path) => Config::from_file(path),
        None => Ok(Config::find_and_load()?.unwrap_or_default()),
    }
}

/// The configured SQL dialect (PostgreSQL by default)
fn config_dialect(config: &Config) -> Result<SqlDialect> {
    match &config.dialect {
        Some(d) => d.parse().map_err(|e: String| miette::miette!(e)),
        None => Ok(SqlDialect::default()),
    }
}

/// Schema files from `schema` (glob patterns are expanded) followed by every
/// `.sql` file under `schema_dir`, in filename order
fn schema_files(config: &Config) -> Result<Vec<PathBuf>> {
    let mut schema_files: Vec<PathBuf> = Vec::new();
    for pattern in &config.schema {
        if is_glob(pattern) {
            let matches = expand_glob(pattern)?;
            if matches.is_empty() {
                miette::bail!("No schema files match pattern '{}'", pattern);
            }
            schema_files.extend(matches);
        } else {
            schema_files.push(PathBuf::from(pattern));
        }
    }

    if let Some(dir) = &config.schema_dir {
        if !Path::new(dir).is_dir() {
            miette::bail!("Schema directory not found: {}", dir);
        }
        // Rollback migrations (`*.down.sql`, Flyway `U*__*.sql`) are not schema
        let matches: Vec<PathBuf> = expand_glob(&format!("{dir}/**/*.sql"))?
            .into_iter()
            .filter(|path| !is_rollback_migration(path))
            .collect();
        if matches.is_empty() {
            miette::bail!("No .sql files found in schema directory {}", dir);
        }
        schema_files.extend(matches);
    }

    if schema_files.is_empty() {
        miette::bail!(
            "No schema files specified. Use --schema, --schema-dir, or configure in sqlsift.toml"
        );
    }
    Ok(schema_files)
}

/// Build the catalog from the schema files. Schema warnings are printed to
/// stderr; a file with errors is returned with its diagnostics instead.
fn build_catalog(
    schema_files: &[PathBuf],
    dialect: SqlDialect,
) -> Result<std::result::Result<Catalog, FileDiagnostics>> {
    let mut builder = SchemaBuilder::with_dialect(dialect);
    for schema_file in schema_files {
        let content = read_file(schema_file)?;
        if let Err(diags) = builder.parse(&content) {
            return Ok(Err(FileDiagnostics {
                file: schema_file.display().to_string(),
                source: content,
                diagnostics: diags,
            }));
        }
    }
    let (catalog, schema_diags) = builder.build();

    if !schema_diags.is_empty() {
        eprintln!(
            "Warning: Schema parsing produced {} warning(s):",
            schema_diags.len()
        );
        for diag in &schema_diags {
            eprintln!("  - {}", diag.message);
        }
    }
    Ok(Ok(catalog))
}

fn run(args: Args) -> Result<bool> {
    let quiet = args.quiet;

    match args.command {
        Command::Check {
            files,
            schema,
            schema_dir,
            ignore,
            config: config_path,
            allow,
            warn,
            deny,
            dialect,
            format,
            max_errors,
            max_warnings,
            stdin_filename,
        } => {
            // Load configuration; CLI args take precedence over the config file
            let config = load_config(config_path.as_deref())?.merge_with_args(
                &schema,
                schema_dir.as_deref(),
                &files,
                &ignore,
                format,
                dialect.as_deref(),
                max_warnings,
            );
            tracing::info!(
                schema_count = config.schema.len(),
                query_pattern_count = config.files.len(),
                "Loaded sqlsift configuration"
            );

            let rules = config.rule_config(&RuleFlags {
                allow: &allow,
                warn: &warn,
                deny: &deny,
            })?;

            let dialect = config_dialect(&config)?;

            // Determine output format
            let output_format = match config.format.as_deref() {
                None | Some("human") => OutputFormat::Human,
                Some("json") => OutputFormat::Json,
                Some("sarif") => OutputFormat::Sarif,
                Some("github") => OutputFormat::Github,
                Some(other) => {
                    return Err(miette::miette!(
                        "Invalid format '{}'. Supported formats: human, json, sarif, github.",
                        other
                    ))
                }
            };

            let schema_files = schema_files(&config)?;
            let formatter = OutputFormatter::new(output_format);
            let catalog = match build_catalog(&schema_files, dialect)? {
                Ok(catalog) => catalog,
                Err(failed) => {
                    formatter.print(&[failed]);
                    return Ok(true);
                }
            };

            // Collect query files from config or CLI (glob patterns are expanded)
            let mut query_files = Vec::new();
            let mut unmatched = Vec::new();
            for pattern in &config.files {
                if is_glob(pattern) {
                    let matches = expand_glob(pattern)?;
                    if matches.is_empty() {
                        unmatched.push(format!("'{pattern}'"));
                    }
                    query_files.extend(matches);
                } else {
                    query_files.push(PathBuf::from(pattern));
                }
            }

            if query_files.is_empty() {
                if !unmatched.is_empty() {
                    miette::bail!("No files match {}", unmatched.join(", "));
                }
                miette::bail!("No query files specified. Use positional arguments or configure in sqlsift.toml");
            }
            // A typo in one of several patterns shouldn't go unnoticed
            for pattern in &unmatched {
                eprintln!("Warning: no files match {pattern}");
            }

            // The query read from stdin (`-`)
            let stdin_count = query_files.iter().filter(|p| is_stdin(p)).count();
            if stdin_count > 1 {
                miette::bail!("'-' (stdin) can only be given once");
            }
            if stdin_count == 0 && stdin_filename.is_some() {
                miette::bail!(
                    "--stdin-filename requires '-' among the files to read the query from stdin"
                );
            }
            let stdin = if stdin_count == 1 {
                let mut content = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut content)
                    .map_err(|e| miette::miette!("Failed to read stdin: {}", e))?;
                Some(content)
            } else {
                None
            };
            let display_name = |path: &Path| -> String {
                if stdin.is_some() && is_stdin(path) {
                    stdin_filename.as_ref().map_or_else(
                        || STDIN_DEFAULT_NAME.to_string(),
                        |name| name.display().to_string(),
                    )
                } else {
                    path.display().to_string()
                }
            };

            // Skip ignored files (`ignore` in sqlsift.toml and `--ignore`); patterns
            // from the config file were already made relative to the current directory
            let ignore_patterns = IgnorePatterns::new(Path::new(""), &config.ignore)
                .map_err(|e| miette::miette!(e))?;
            let found = query_files.len();
            query_files.retain(|path| {
                let ignored = ignore_patterns.is_ignored(path);
                if ignored {
                    tracing::debug!(file = %path.display(), "Ignoring file");
                }
                !ignored
            });
            let ignored_count = found - query_files.len();
            if query_files.is_empty() {
                if !quiet {
                    eprintln!("No files to check ({ignored_count} ignored)");
                }
                formatter.print(&[]);
                return Ok(false);
            }

            // Analyze the query files in parallel; results are then collected in file
            // order, so output and --max-errors behave exactly as when run sequentially
            let analyzed = analyze_files(&query_files, stdin.as_deref(), &catalog, dialect, &rules);

            let mut total_errors = 0;
            let mut total_warnings = 0;
            let mut files_checked = 0;
            let mut results = Vec::new();
            let max_errors = if max_errors == 0 {
                usize::MAX
            } else {
                max_errors
            };
            let mut limit_reached = false;

            for (query_file, analyzed) in query_files.iter().zip(analyzed) {
                if total_errors >= max_errors {
                    limit_reached = true;
                    break;
                }

                let (content, diagnostics) = analyzed?;
                files_checked += 1;

                let mut diagnostics_to_print = Vec::new();
                for diag in diagnostics {
                    if matches!(diag.severity, sqlsift_core::Severity::Error)
                        && total_errors >= max_errors
                    {
                        limit_reached = true;
                        break;
                    }

                    match diag.severity {
                        sqlsift_core::Severity::Error => total_errors += 1,
                        sqlsift_core::Severity::Warning => total_warnings += 1,
                        sqlsift_core::Severity::Info => {}
                    }
                    diagnostics_to_print.push(diag);
                }

                results.push(FileDiagnostics {
                    file: display_name(query_file),
                    source: content,
                    diagnostics: diagnostics_to_print,
                });

                if limit_reached {
                    break;
                }
            }

            formatter.print(&results);

            // Print summary
            if !quiet {
                if limit_reached && max_errors != usize::MAX {
                    let unchecked = query_files.len() - files_checked;
                    if unchecked > 0 {
                        eprintln!(
                            "Reached maximum error limit ({max_errors}). Stopped early. {unchecked} file(s) not checked."
                        );
                    } else {
                        eprintln!("Reached maximum error limit ({max_errors}). Stopped early.");
                    }
                }

                if total_errors > 0 || total_warnings > 0 {
                    eprintln!();
                    eprintln!(
                        "Found {total_errors} error(s), {total_warnings} warning(s) in {files_checked} file(s)"
                    );
                } else {
                    eprintln!("All {files_checked} file(s) passed validation");
                }
            }

            // Too many warnings fail the check; the reason is printed even with
            // --quiet, as it explains the exit code
            let too_many_warnings = config.max_warnings.filter(|max| total_warnings > *max);
            if let Some(max) = too_many_warnings {
                let origin = if max_warnings.is_some() {
                    "--max-warnings"
                } else {
                    "max_warnings in sqlsift.toml"
                };
                eprintln!(
                    "Too many warnings: {total_warnings} found, the maximum is {max} ({origin})"
                );
            }

            Ok(total_errors > 0 || too_many_warnings.is_some())
        }

        Command::Rules => {
            print_rules();
            Ok(false)
        }

        Command::Schema {
            files,
            mut schema,
            schema_dir,
            config: config_path,
            dialect,
            format,
        } => {
            // Positional files are schema files too (kept for backward compatibility)
            schema.extend(files);
            let config = load_config(config_path.as_deref())?.merge_with_args(
                &schema,
                schema_dir.as_deref(),
                &[],
                &[],
                None,
                dialect.as_deref(),
                None,
            );
            let dialect = config_dialect(&config)?;
            let schema_files = schema_files(&config)?;

            let catalog = match build_catalog(&schema_files, dialect)? {
                Ok(catalog) => catalog,
                Err(failed) => {
                    let output_format = match format {
                        SchemaFormat::Human => OutputFormat::Human,
                        SchemaFormat::Json => OutputFormat::Json,
                    };
                    OutputFormatter::new(output_format).print(&[failed]);
                    return Ok(true);
                }
            };

            let report = SchemaReport {
                catalog: &catalog,
                dialect,
                schema_files: &schema_files,
            };
            print!("{}", report.render(format));
            Ok(false)
        }

        Command::Parse { file } => {
            use sqlparser::parser::Parser;

            // Parse and display AST (for debugging)
            let content = read_file(&file)?;

            let dialect = SqlDialect::default().parser_dialect();
            match Parser::parse_sql(dialect.as_ref(), &content) {
                Ok(statements) => {
                    for (i, stmt) in statements.iter().enumerate() {
                        println!("Statement {}:", i + 1);
                        println!("{stmt:#?}");
                        println!();
                    }
                }
                Err(e) => {
                    eprintln!("Parse error: {e}");
                    return Ok(true);
                }
            }

            Ok(false)
        }
    }
}
