//! sqlsift CLI - SQL static analysis tool

mod args;
mod config;
mod output;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use miette::Result;
use sqlsift_core::schema::{is_rollback_migration, SchemaBuilder};
use sqlsift_core::{Analyzer, Diagnostic, RuleConfig, SqlDialect};

use crate::args::{Args, Command, OutputFormat};
use crate::config::{Config, RuleFlags};
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
            eprintln!("Error: {:?}", e);
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

/// Read and analyze each query file, in parallel across the available cores.
/// Results are returned in the same order as `files`.
fn analyze_files(
    files: &[PathBuf],
    catalog: &sqlsift_core::schema::Catalog,
    dialect: SqlDialect,
    rules: &RuleConfig,
) -> Vec<AnalyzedFile> {
    let analyze_one = |path: &PathBuf| -> AnalyzedFile {
        tracing::debug!(file = %path.display(), "Analyzing SQL file");
        let content = read_file(path)?;
        let diagnostics = Analyzer::with_dialect(catalog, dialect)
            .with_rules(rules.clone())
            .analyze(&content);
        Ok((content, diagnostics))
    };

    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
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

fn run(args: Args) -> Result<bool> {
    let quiet = args.quiet;

    match args.command {
        Command::Check {
            files,
            schema,
            schema_dir,
            config: config_path,
            allow,
            warn,
            deny,
            dialect,
            format,
            max_errors,
        } => {
            // Load configuration
            let config = if let Some(path) = config_path {
                // Load from specified path
                Config::from_file(&path)?
            } else {
                // Try to find sqlsift.toml
                Config::find_and_load()?.unwrap_or_default()
            };

            // Merge CLI args with config (CLI takes precedence)
            let config = config.merge_with_args(&schema, &schema_dir, &files, &format, &dialect);
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

            // Parse and validate dialect
            let dialect: SqlDialect = match &config.dialect {
                Some(d) => d.parse().map_err(|e: String| miette::miette!(e))?,
                None => SqlDialect::default(),
            };

            // Determine output format
            let output_format = match config.format.as_deref() {
                None | Some("human") => OutputFormat::Human,
                Some("json") => OutputFormat::Json,
                Some("sarif") => OutputFormat::Sarif,
                Some(other) => {
                    return Err(miette::miette!(
                        "Invalid format '{}'. Supported formats: human, json, sarif.",
                        other
                    ))
                }
            };

            // Get schema files from config or CLI (glob patterns are expanded)
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
                let matches: Vec<PathBuf> = expand_glob(&format!("{}/**/*.sql", dir))?
                    .into_iter()
                    .filter(|path| !is_rollback_migration(path))
                    .collect();
                if matches.is_empty() {
                    miette::bail!("No .sql files found in schema directory {}", dir);
                }
                schema_files.extend(matches);
            }

            if schema_files.is_empty() {
                miette::bail!("No schema files specified. Use --schema, --schema-dir, or configure in sqlsift.toml");
            }

            let formatter = OutputFormatter::new(output_format);

            // Build schema catalog
            let mut builder = SchemaBuilder::with_dialect(dialect);
            for schema_file in &schema_files {
                let content = read_file(schema_file)?;
                if let Err(diags) = builder.parse(&content) {
                    formatter.print(&[FileDiagnostics {
                        file: schema_file.display().to_string(),
                        source: content,
                        diagnostics: diags,
                    }]);
                    return Ok(true);
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

            // Collect query files from config or CLI (glob patterns are expanded)
            let mut query_files = Vec::new();
            for pattern in &config.files {
                if is_glob(pattern) {
                    query_files.extend(expand_glob(pattern)?);
                } else {
                    query_files.push(PathBuf::from(pattern));
                }
            }

            if query_files.is_empty() {
                miette::bail!("No query files specified. Use positional arguments or configure in sqlsift.toml");
            }

            // Analyze the query files in parallel; results are then collected in file
            // order, so output and --max-errors behave exactly as when run sequentially
            let analyzed = analyze_files(&query_files, &catalog, dialect, &rules);

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
                        _ => {}
                    }
                    diagnostics_to_print.push(diag);
                }

                results.push(FileDiagnostics {
                    file: query_file.display().to_string(),
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
                        "Found {} error(s), {} warning(s) in {} file(s)",
                        total_errors, total_warnings, files_checked
                    );
                } else {
                    eprintln!("All {} file(s) passed validation", files_checked);
                }
            }

            Ok(total_errors > 0)
        }

        Command::Rules => {
            print_rules();
            Ok(false)
        }

        Command::Schema { files } => {
            // Build and display schema information
            let mut builder = SchemaBuilder::new();
            for schema_file in &files {
                let content = read_file(schema_file)?;
                let _ = builder.parse(&content);
            }
            let (catalog, _) = builder.build();

            println!("Schema Information:");
            println!("==================");
            for (schema_name, schema) in &catalog.schemas {
                println!("\nSchema: {}", schema_name);
                for (table_name, table) in &schema.tables {
                    println!("  Table: {}", table_name);
                    for (col_name, col) in &table.columns {
                        let nullable = if col.nullable { "NULL" } else { "NOT NULL" };
                        println!(
                            "    - {} {} {}",
                            col_name,
                            col.data_type.display_name(),
                            nullable
                        );
                    }
                }
            }

            Ok(false)
        }

        Command::Parse { file } => {
            // Parse and display AST (for debugging)
            let content = read_file(&file)?;

            use sqlparser::parser::Parser;

            let dialect = SqlDialect::default().parser_dialect();
            match Parser::parse_sql(dialect.as_ref(), &content) {
                Ok(statements) => {
                    for (i, stmt) in statements.iter().enumerate() {
                        println!("Statement {}:", i + 1);
                        println!("{:#?}", stmt);
                        println!();
                    }
                }
                Err(e) => {
                    eprintln!("Parse error: {}", e);
                    return Ok(true);
                }
            }

            Ok(false)
        }
    }
}
