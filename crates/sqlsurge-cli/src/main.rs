//! sqlsurge CLI - SQL static analysis tool

mod args;
mod config;
mod output;

use std::fs;
use std::process::ExitCode;

use clap::Parser;
use miette::{IntoDiagnostic, Result};
use sqlsurge_core::schema::SchemaBuilder;
use sqlsurge_core::{Analyzer, SqlDialect};

use crate::args::{Args, Command, OutputFormat};
use crate::config::Config;
use crate::output::OutputFormatter;

fn main() -> ExitCode {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    let args = Args::parse();

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

fn run(args: Args) -> Result<bool> {
    match args.command {
        Command::Check {
            files,
            schema,
            schema_dir,
            config: config_path,
            disable,
            dialect,
            format,
            ..
        } => {
            // Parse and validate dialect
            let dialect: SqlDialect = dialect.parse().map_err(|e: String| miette::miette!(e))?;
            // Load configuration
            let config = if let Some(path) = config_path {
                // Load from specified path
                Config::from_file(&path)?
            } else {
                // Try to find sqlsurge.toml
                Config::find_and_load()?.unwrap_or_default()
            };

            // Merge CLI args with config (CLI takes precedence)
            let config = config.merge_with_args(&schema, &schema_dir, &files, &format, &disable);

            // Get schema files from config or CLI
            let mut schema_files: Vec<std::path::PathBuf> =
                config.schema.iter().map(std::path::PathBuf::from).collect();

            if let Some(dir) = &config.schema_dir {
                let pattern = format!("{}/**/*.sql", dir);
                for path in glob::glob(&pattern).into_diagnostic()?.flatten() {
                    schema_files.push(path);
                }
            }

            if schema_files.is_empty() {
                miette::bail!("No schema files specified. Use --schema, --schema-dir, or configure in sqlsurge.toml");
            }

            // Determine output format
            let output_format = if let Some(fmt_str) = &config.format {
                match fmt_str.as_str() {
                    "json" => OutputFormat::Json,
                    "sarif" => OutputFormat::Sarif,
                    _ => OutputFormat::Human,
                }
            } else {
                OutputFormat::Human
            };

            // Build schema catalog
            let mut builder = SchemaBuilder::with_dialect(dialect);
            for schema_file in &schema_files {
                let content = fs::read_to_string(schema_file).into_diagnostic()?;
                if let Err(diags) = builder.parse(&content) {
                    let formatter =
                        OutputFormatter::new(output_format, schema_file.display().to_string());
                    formatter.print_diagnostics(&diags, &content);
                    return Ok(true);
                }
            }
            let (catalog, schema_diags) = builder.build();

            if !schema_diags.is_empty() {
                eprintln!(
                    "Warning: Schema parsing produced {} warnings",
                    schema_diags.len()
                );
            }

            // Collect query files from config or CLI
            let mut query_files = Vec::new();
            let file_patterns: Vec<std::path::PathBuf> = if !config.files.is_empty() {
                config.files.iter().map(std::path::PathBuf::from).collect()
            } else {
                vec![]
            };

            for pattern in &file_patterns {
                let pattern_str = pattern.display().to_string();
                if pattern_str.contains('*') {
                    for path in glob::glob(&pattern_str).into_diagnostic()?.flatten() {
                        query_files.push(path);
                    }
                } else {
                    query_files.push(pattern.clone());
                }
            }

            if query_files.is_empty() {
                miette::bail!("No query files specified. Use positional arguments or configure in sqlsurge.toml");
            }

            // Analyze each query file
            let mut total_errors = 0;
            let mut total_warnings = 0;
            let mut analyzer = Analyzer::with_dialect(&catalog, dialect);

            // Get disabled rules
            let disabled_rules: std::collections::HashSet<String> =
                config.disable.iter().cloned().collect();

            for query_file in &query_files {
                let content = fs::read_to_string(query_file).into_diagnostic()?;
                let diagnostics = analyzer.analyze(&content);

                // Filter out disabled rules
                let filtered_diagnostics: Vec<_> = diagnostics
                    .into_iter()
                    .filter(|d| !disabled_rules.contains(d.code()))
                    .collect();

                if !filtered_diagnostics.is_empty() {
                    let formatter =
                        OutputFormatter::new(output_format, query_file.display().to_string());
                    formatter.print_diagnostics(&filtered_diagnostics, &content);

                    for diag in &filtered_diagnostics {
                        match diag.severity {
                            sqlsurge_core::Severity::Error => total_errors += 1,
                            sqlsurge_core::Severity::Warning => total_warnings += 1,
                            _ => {}
                        }
                    }
                }
            }

            // Print summary
            if total_errors > 0 || total_warnings > 0 {
                eprintln!();
                eprintln!(
                    "Found {} error(s), {} warning(s) in {} file(s)",
                    total_errors,
                    total_warnings,
                    query_files.len()
                );
            } else {
                eprintln!("All {} file(s) passed validation", query_files.len());
            }

            Ok(total_errors > 0)
        }

        Command::Schema { files } => {
            // Build and display schema information
            let mut builder = SchemaBuilder::new();
            for schema_file in &files {
                let content = fs::read_to_string(schema_file).into_diagnostic()?;
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
            let content = fs::read_to_string(&file).into_diagnostic()?;

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
