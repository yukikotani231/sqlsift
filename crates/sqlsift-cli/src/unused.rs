//! `sqlsift unused`: the tables and columns that no query uses

use miette::Result;
use sqlsift_core::error::DiagnosticKind;
use sqlsift_core::unused::{Unused, Usage};
use sqlsift_core::{Analyzer, RuleConfig};

use crate::args::OutputFormat;
use crate::config::Config;
use crate::{
    analyze_files, build_catalog, collect_query_files, load_dbt_catalogs, read_file, QueryFiles,
    QueryOptions,
};

/// Run `sqlsift unused`. Returns whether anything is unused.
pub fn run(config: &Config, exclude: &[String], quiet: bool) -> Result<bool> {
    let dialect = crate::config_dialect(config)?;
    let templating = config.templating()?;
    let format = crate::output_format(config)?;
    if !matches!(format, OutputFormat::Human | OutputFormat::Json) {
        miette::bail!("`sqlsift unused` writes human or json output");
    }
    let mut exclude_patterns = Vec::new();
    for pattern in config.unused_exclude.iter().chain(exclude) {
        let compiled = glob::Pattern::new(pattern)
            .map_err(|e| miette::miette!("Invalid exclude pattern '{pattern}': {e}"))?;
        exclude_patterns.push(compiled);
    }

    let mut dbt_projects = crate::dbt::DbtProjects::default();
    let dbt_catalogs = crate::dbt_catalog_files(config, templating, &mut dbt_projects)?;
    let schema_files = crate::schema_files(config, !dbt_catalogs.is_empty())?;
    let mut catalog = match build_catalog(&schema_files, dialect, !quiet)? {
        Ok(catalog) => catalog,
        Err(failed) => {
            crate::output::OutputFormatter::new(format).print(&[failed]);
            miette::bail!("The schema has errors");
        }
    };
    load_dbt_catalogs(&mut catalog, &dbt_catalogs)?;

    let QueryFiles {
        files,
        templating: file_templating,
        ..
    } = collect_query_files(
        config,
        templating,
        &mut dbt_projects,
        None,
        "Give query files or configure `files` in sqlsift.toml",
    )?;

    let mut usage = Usage::default();
    let mut incomplete: Vec<String> = Vec::new();

    // The schema's views (and CREATE TABLE ... AS) read their tables' columns
    for path in &schema_files {
        let content = read_file(path)?;
        let mut analyzer = Analyzer::with_dialect(&catalog, dialect).with_references();
        analyzer.analyze(&content);
        usage.add(&path.display().to_string(), analyzer.references(), &catalog);
    }
    let skipped_views = catalog
        .skipped_definitions
        .iter()
        .filter(|d| d.kind.contains("VIEW"))
        .count();
    if skipped_views > 0 {
        incomplete.push(format!(
            "{skipped_views} view(s) in the schema could not be parsed"
        ));
    }
    if !catalog.user_functions.is_empty() {
        incomplete.push(format!(
            "{} function(s) or procedure(s) in the schema are not analyzed",
            catalog.user_functions.len()
        ));
    }

    let rules = RuleConfig::default();
    let embedded_sql_tags = config.embedded_sql_tags();
    let embedded_sql_macros = config.embedded_sql_macros();
    let embedded_sql_functions = config.embedded_sql_functions();
    let embedded_sql_go_functions = config.embedded_sql_go_functions();
    let options = QueryOptions {
        catalog: &catalog,
        dialect,
        templating: &file_templating,
        rules: &rules,
        embedded_sql_tags: &embedded_sql_tags,
        embedded_sql_macros: &embedded_sql_macros,
        embedded_sql_functions: &embedded_sql_functions,
        embedded_sql_go_functions: &embedded_sql_go_functions,
        stdin_filename: None,
        references: true,
    };
    let mut unparsed = Vec::new();
    for (path, analyzed) in files.iter().zip(analyze_files(&files, None, &options)) {
        let (_, diagnostics, references) = analyzed?;
        let file = path.display().to_string();
        if diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::ParseError)
        {
            unparsed.push(file.clone());
        }
        usage.add(&file, &references, &catalog);
    }
    if !unparsed.is_empty() {
        incomplete.push(format!(
            "{} query file(s) have statements that could not be parsed: {}",
            unparsed.len(),
            unparsed.join(", ")
        ));
    }

    let unused: Vec<Unused> = usage
        .unused(&catalog)
        .into_iter()
        .filter(|u| !excluded(u, &exclude_patterns))
        .collect();

    match format {
        OutputFormat::Json => print_json(&unused, &incomplete, files.len()),
        _ => print_human(&unused, &incomplete, files.len(), quiet),
    }
    Ok(!unused.is_empty())
}

/// Whether an `exclude` pattern matches the table (with or without its schema),
/// or the column
fn excluded(unused: &Unused, patterns: &[glob::Pattern]) -> bool {
    let table = unused.table.as_str();
    let bare = table.rsplit('.').next().unwrap_or(table);
    let mut names = vec![table.to_string(), bare.to_string()];
    if let Some(column) = &unused.column {
        names.push(format!("{table}.{column}"));
        names.push(format!("{bare}.{column}"));
    }
    let options = glob::MatchOptions {
        case_sensitive: false,
        ..glob::MatchOptions::new()
    };
    patterns
        .iter()
        .any(|p| names.iter().any(|n| p.matches_with(n, options)))
}

fn describe(unused: &Unused) -> String {
    match (&unused.column, unused.writes) {
        (None, _) => "never referenced".to_string(),
        (Some(_), 0) => "never referenced".to_string(),
        (Some(_), n) => format!("written ({n} statement(s)), never read"),
    }
}

fn print_human(unused: &[Unused], incomplete: &[String], files: usize, quiet: bool) {
    let width = unused.iter().map(|u| u.name().len()).max().unwrap_or(0);
    for item in unused {
        println!("{:width$}  {}", item.name(), describe(item));
    }
    if quiet {
        return;
    }
    for reason in incomplete {
        eprintln!("Incomplete: {reason}");
    }
    let tables = unused.iter().filter(|u| u.column.is_none()).count();
    let columns = unused.len() - tables;
    if unused.is_empty() {
        eprintln!("Every table and column is used by the {files} query file(s)");
    } else {
        eprintln!(
            "{tables} table(s) and {columns} column(s) unused by the {files} query file(s). \
             They may be used outside them (an ORM, another service, a BI tool): \
             exclude those with `unused_exclude` in sqlsift.toml"
        );
    }
}

fn print_json(unused: &[Unused], incomplete: &[String], files: usize) {
    let items: Vec<serde_json::Value> = unused
        .iter()
        .map(|u| {
            let mut object = serde_json::json!({
                "table": u.table,
                "status": if u.writes > 0 { "written" } else { "unreferenced" },
            });
            if let Some(column) = &u.column {
                object["column"] = column.clone().into();
                object["writes"] = u.writes.into();
            }
            object
        })
        .collect();
    let report = serde_json::json!({
        "unused": items,
        "files": files,
        "incomplete": incomplete,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
}
