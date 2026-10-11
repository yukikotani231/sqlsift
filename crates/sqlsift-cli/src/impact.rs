//! `sqlsift impact`: the queries a schema change breaks or fixes

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use miette::Result;
use sqlsift_core::analyzer::{Reference, ReferenceKind};
use sqlsift_core::impact::{diff_diagnostics, SchemaChanges};
use sqlsift_core::{Catalog, Severity};

use crate::args::OutputFormat;
use crate::config::Config;
use crate::output::{self, FileDiagnostics, OutputFormatter};
use crate::{
    analyze_files, build_catalog, collect_query_files, expand_glob, is_glob, load_dbt_catalogs,
    load_dbt_vars, read_file, QueryFiles, QueryOptions,
};

/// What `sqlsift impact` runs with, besides the configuration
pub struct ImpactOptions<'a> {
    pub migrations: &'a [PathBuf],
    /// `--base`: also the schema files changed since this git revision
    pub base: Option<&'a str>,
    pub rules: &'a sqlsift_core::RuleConfig,
    pub quiet: bool,
    /// `--references`: also list the queries that use a changed table or view
    /// without a diagnostic changing
    pub references: bool,
}

/// The schema files before and after the migrations
#[derive(Debug, PartialEq)]
struct SchemaVersions {
    before: Vec<PathBuf>,
    after: Vec<PathBuf>,
}

/// Split the schema files at the migrations: "before" stops at the first
/// migration among them, "after" is every schema file followed by the
/// migrations that are not among them
fn schema_versions(schema_files: &[PathBuf], migrations: &[PathBuf]) -> SchemaVersions {
    let key = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let schema_keys: Vec<PathBuf> = schema_files.iter().map(|p| key(p)).collect();
    let migration_keys: Vec<PathBuf> = migrations.iter().map(|p| key(p)).collect();
    let first = schema_keys
        .iter()
        .position(|k| migration_keys.contains(k))
        .unwrap_or(schema_files.len());
    let mut after = schema_files.to_vec();
    for (migration, k) in migrations.iter().zip(&migration_keys) {
        if !schema_keys.contains(k) && !after.iter().any(|p| key(p) == *k) {
            after.push(migration.clone());
        }
    }
    SchemaVersions {
        before: schema_files[..first].to_vec(),
        after,
    }
}

/// Expand the migration arguments (glob patterns are expanded)
fn migration_files(patterns: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for pattern in patterns {
        let text = pattern.to_string_lossy();
        if is_glob(&text) {
            let matches = expand_glob(&text)?;
            if matches.is_empty() {
                miette::bail!("No migration files match '{}'", text);
            }
            files.extend(matches);
        } else if pattern.is_file() {
            files.push(pattern.clone());
        } else {
            miette::bail!("Migration file not found: {}", pattern.display());
        }
    }
    Ok(files)
}

/// Run git in the current directory, returning its standard output
fn git(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| miette::miette!("Failed to run git: {}", e))?;
    if !output.status.success() {
        miette::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The schema files added or changed since the merge base of `base` and
/// `HEAD` (`base` itself when git can't find one), in schema file order:
/// committed, uncommitted and untracked changes
fn changed_schema_files(base: &str, schema_files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let root = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?.trim_end());
    // A shallow checkout (CI) may not have the history to find the merge
    // base: compare with the revision itself then (on a pull request's merge
    // commit, the base branch tip is the merge base anyway)
    let merge_base = git(&["merge-base", base, "HEAD"]).unwrap_or_else(|_| base.to_string());
    let diff = git(&[
        "diff",
        "--name-only",
        "-z",
        "--diff-filter=AMR",
        merge_base.trim_end(),
    ])?;
    let untracked = git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--full-name",
        ":/",
    ])?;
    let changed: Vec<PathBuf> = diff
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|name| !name.is_empty())
        .map(|name| fs::canonicalize(root.join(name)).unwrap_or_else(|_| root.join(name)))
        .collect();
    Ok(schema_files
        .iter()
        .filter(|path| fs::canonicalize(path).is_ok_and(|p| changed.contains(&p)))
        .cloned()
        .collect())
}

/// Run `sqlsift impact`. Returns whether the change introduces errors.
pub fn run(config: &Config, options: &ImpactOptions) -> Result<bool> {
    let dialect = crate::config_dialect(config)?;
    let templating = config.templating()?;
    let format = crate::output_format(config)?;
    let formatter = OutputFormatter::new(format);

    let migrations = migration_files(options.migrations)?;
    let mut dbt_projects = crate::dbt::DbtProjects::default();
    let dbt_catalogs = crate::dbt_catalog_files(config, templating, &mut dbt_projects)?;
    // Without schema files the migrations are the whole schema
    let schema_files = crate::schema_files(config, true)?;
    let mut migrations = migrations;
    if let Some(base) = options.base {
        for file in changed_schema_files(base, &schema_files)? {
            if !migrations.contains(&file) {
                migrations.push(file);
            }
        }
    }
    let versions = schema_versions(&schema_files, &migrations);

    let mut catalogs: Vec<Catalog> = Vec::with_capacity(2);
    for (files, warn) in [(&versions.before, false), (&versions.after, true)] {
        let mut catalog = match build_catalog(files, dialect, warn)? {
            Ok(catalog) => catalog,
            Err(failed) => {
                formatter.print(&[failed]);
                return Ok(true);
            }
        };
        load_dbt_catalogs(&mut catalog, &dbt_catalogs)?;
        load_dbt_vars(&mut catalog, config, templating, &mut dbt_projects);
        catalogs.push(catalog);
    }
    let (before, after) = (&catalogs[0], &catalogs[1]);
    let changes = SchemaChanges::between(before, after);

    let QueryFiles {
        files,
        templating: file_templating,
        ..
    } = collect_query_files(
        config,
        templating,
        &mut dbt_projects,
        None,
        "Use --queries or configure `files` in sqlsift.toml",
    )?;

    // Only files that name a changed table or view can be affected
    let mut affected = Vec::new();
    let mut affected_templating = Vec::new();
    for (path, templating) in files.iter().zip(&file_templating) {
        if changes.may_affect(&read_file(path)?) {
            affected.push(path.clone());
            affected_templating.push(*templating);
        }
    }

    let embedded_sql_tags = config.embedded_sql_tags();
    let embedded_sql_macros = config.embedded_sql_macros();
    let analyze = |catalog: &Catalog| {
        let options = QueryOptions {
            catalog,
            dialect,
            templating: &affected_templating,
            rules: options.rules,
            embedded_sql_tags: &embedded_sql_tags,
            embedded_sql_macros: &embedded_sql_macros,
            stdin_filename: None,
            references: options.references,
        };
        analyze_files(&affected, None, &options)
    };
    let analyzed_before = analyze(before);
    let analyzed_after = analyze(after);

    let changed_names: HashSet<String> = changes.relations.iter().map(|r| r.name.clone()).collect();
    let mut introduced = Vec::new();
    let mut resolved = Vec::new();
    let mut referencing = Vec::new();
    for ((path, old), new) in affected.iter().zip(analyzed_before).zip(analyzed_after) {
        let (source, old, old_references) = old?;
        let (_, new, new_references) = new?;
        let diff = diff_diagnostics(old, new);
        let file = path.display().to_string();
        if options.references {
            // Lines whose diagnostics change are reported already
            let reported: HashSet<usize> = diff
                .introduced
                .iter()
                .chain(&diff.resolved)
                .filter_map(|d| d.span.map(|s| s.line))
                .collect();
            let lines = referencing_lines(
                old_references.iter().chain(&new_references),
                &changed_names,
                &reported,
            );
            if !lines.is_empty() {
                referencing.push((file.clone(), lines));
            }
        }
        if !diff.introduced.is_empty() {
            introduced.push(FileDiagnostics {
                file: file.clone(),
                source: source.clone(),
                diagnostics: diff.introduced,
                ..FileDiagnostics::default()
            });
        }
        if !diff.resolved.is_empty() {
            resolved.push(FileDiagnostics {
                file,
                source,
                diagnostics: diff.resolved,
                ..FileDiagnostics::default()
            });
        }
    }

    let introduced_errors = count(&introduced, Severity::Error);
    match format {
        OutputFormat::Human => {
            if !options.quiet {
                print_changes(&changes, &migrations);
            }
            formatter.print(&introduced);
            if !options.quiet {
                print_resolved(&resolved);
                if options.references {
                    print_references(&referencing);
                }
                print_summary(&introduced, &resolved, affected.len(), files.len());
            }
        }
        OutputFormat::Json => print_json(
            &changes,
            &migrations,
            &introduced,
            &resolved,
            options.references.then_some(referencing.as_slice()),
        ),
        // Annotations and code scanning results show what the change breaks
        OutputFormat::Sarif | OutputFormat::Github => formatter.print(&introduced),
    }
    Ok(introduced_errors > 0)
}

/// The uses of changed tables and views on one line of a query file:
/// `schema.table` or `schema.table.column` names, sorted
type ReferencingLine = (usize, Vec<String>);

/// The lines of a query file that use a changed table or view, other than the
/// `reported` lines
fn referencing_lines<'r>(
    references: impl Iterator<Item = &'r Reference>,
    changed: &HashSet<String>,
    reported: &HashSet<usize>,
) -> Vec<ReferencingLine> {
    let mut lines: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for reference in references {
        let relation = reference.relation.to_string();
        if !changed.contains(&relation) || reported.contains(&reference.span.line) {
            continue;
        }
        let name = match &reference.kind {
            ReferenceKind::Relation => relation,
            ReferenceKind::Column(column) => format!("{relation}.{column}"),
            ReferenceKind::AllColumns => format!("{relation}.*"),
        };
        lines.entry(reference.span.line).or_default().insert(name);
    }
    lines
        .into_iter()
        .map(|(line, names)| (line, names.into_iter().collect()))
        .collect()
}

fn print_references(referencing: &[(String, Vec<ReferencingLine>)]) {
    if referencing.is_empty() {
        return;
    }
    eprintln!("Also using the changed tables and views (no diagnostic changes):");
    for (file, lines) in referencing {
        for (line, names) in lines {
            eprintln!("  {file}:{line}  {}", names.join(", "));
        }
    }
    eprintln!();
}

fn count(files: &[FileDiagnostics], severity: Severity) -> usize {
    files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .filter(|d| d.severity == severity)
        .count()
}

fn print_changes(changes: &SchemaChanges, migrations: &[PathBuf]) {
    if migrations.is_empty() {
        eprintln!("No schema files changed");
        return;
    }
    let names: Vec<String> = migrations.iter().map(|m| m.display().to_string()).collect();
    eprintln!("Impact of {}", names.join(", "));
    if changes.is_empty() {
        eprintln!("  no table or view changes");
    }
    for relation in &changes.relations {
        eprintln!(
            "  {} {} {}",
            relation.kind.name(),
            relation.relation,
            relation.name
        );
    }
    if changes.global {
        eprintln!(
            "  other schema changes (types, search path, functions): every query file is checked"
        );
    }
    eprintln!();
}

fn print_resolved(resolved: &[FileDiagnostics]) {
    if resolved.is_empty() {
        return;
    }
    eprintln!("Fixed by the change:");
    for file in resolved {
        for diag in &file.diagnostics {
            let at = output::location(diag, &file.source).map_or_else(
                || file.file.clone(),
                |(l, c)| format!("{}:{l}:{c}", file.file),
            );
            eprintln!("  {at}  {} {}", diag.code(), diag.message);
        }
    }
    eprintln!();
}

fn print_summary(
    introduced: &[FileDiagnostics],
    resolved: &[FileDiagnostics],
    checked: usize,
    total: usize,
) {
    let new: usize = introduced.iter().map(|f| f.diagnostics.len()).sum();
    let fixed: usize = resolved.iter().map(|f| f.diagnostics.len()).sum();
    if new > 0 {
        eprintln!(
            "The change introduces {new} problem(s) in {} file(s) and fixes {fixed}",
            introduced.len()
        );
    } else {
        eprintln!("The change introduces no problems and fixes {fixed}");
    }
    eprintln!(
        "Checked {checked} of {total} query file(s); the others name no changed table or view"
    );
}

/// JSON output: the changed relations, and the diagnostics the change
/// introduces and resolves, per file as in `sqlsift check --format json`
fn print_json(
    changes: &SchemaChanges,
    migrations: &[PathBuf],
    introduced: &[FileDiagnostics],
    resolved: &[FileDiagnostics],
    referencing: Option<&[(String, Vec<ReferencingLine>)]>,
) {
    let refs = |files: &[FileDiagnostics]| -> Vec<serde_json::Value> {
        output::json_files(&files.iter().collect::<Vec<_>>())
    };
    let mut output = serde_json::json!({
        "migrations": migrations.iter().map(|m| m.display().to_string()).collect::<Vec<_>>(),
        "changes": changes.relations,
        "other_changes": changes.global,
        "introduced": refs(introduced),
        "resolved": refs(resolved),
    });
    if let (Some(referencing), Some(object)) = (referencing, output.as_object_mut()) {
        let files: Vec<serde_json::Value> = referencing
            .iter()
            .map(|(file, lines)| {
                let lines: Vec<serde_json::Value> = lines
                    .iter()
                    .map(|(line, names)| serde_json::json!({ "line": line, "uses": names }))
                    .collect();
                serde_json::json!({ "file": file, "lines": lines })
            })
            .collect();
        object.insert("references".into(), files.into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn migration_among_schema_files() {
        let versions = schema_versions(
            &paths(&["m/001.sql", "m/002.sql", "m/003.sql"]),
            &paths(&["m/002.sql"]),
        );
        assert_eq!(versions.before, paths(&["m/001.sql"]));
        assert_eq!(
            versions.after,
            paths(&["m/001.sql", "m/002.sql", "m/003.sql"])
        );
    }

    #[test]
    fn migration_outside_schema_files() {
        let versions = schema_versions(&paths(&["schema.sql"]), &paths(&["new/004.sql"]));
        assert_eq!(versions.before, paths(&["schema.sql"]));
        assert_eq!(versions.after, paths(&["schema.sql", "new/004.sql"]));
    }
}
