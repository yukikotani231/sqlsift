//! `sqlsift init`: detect the project layout and write `sqlsift.toml`

mod detect;
mod sqlc_config;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use miette::Result;
use sqlsift_core::SqlDialect;

pub use detect::{detect, Detection};

/// The configuration file `sqlsift init` writes
pub const CONFIG_FILE: &str = "sqlsift.toml";

const DOCS: &str = "https://sqlsift.dev/docs";

/// Options of `sqlsift init`
pub struct InitOptions {
    /// Project directory
    pub dir: PathBuf,
    /// Overwrite an existing sqlsift.toml
    pub force: bool,
    /// Dialect to write instead of the detected one
    pub dialect: Option<String>,
    /// Don't run a first check
    pub no_check: bool,
    /// Print the configuration instead of writing it
    pub dry_run: bool,
}

/// Run `sqlsift init`. Prints what was detected, writes `sqlsift.toml`, runs a
/// first check and prints the next steps. Diagnostics of the first check don't
/// make it fail.
pub fn run(options: &InitOptions) -> Result<bool> {
    let dir = &options.dir;
    if !dir.is_dir() {
        miette::bail!("Not a directory: {}", dir.display());
    }
    let config_path = dir.join(CONFIG_FILE);
    if config_path.exists() && !options.force && !options.dry_run {
        miette::bail!(
            "{} already exists; run `sqlsift init --force` to overwrite it, or \
             `sqlsift init --dry-run` to print the configuration init would write",
            display(&config_path)
        );
    }
    let dialect_override = match &options.dialect {
        Some(d) => Some(
            d.parse::<SqlDialect>()
                .map_err(|e: String| miette::miette!(e))?
                .to_string(),
        ),
        None => None,
    };

    let detection = detect(dir);
    let config = render(&detection, dialect_override.as_deref());
    let report = report(&detection, dialect_override.as_deref());

    if options.dry_run {
        eprint!("{report}");
        print!("{config}");
        return Ok(false);
    }

    std::fs::write(&config_path, &config)
        .map_err(|e| miette::miette!("Failed to write {}: {}", display(&config_path), e))?;
    print!("{report}");
    println!();
    println!("Wrote {}", display(&config_path));

    let in_dir = if dir == Path::new(".") || dir.as_os_str().is_empty() {
        String::new()
    } else {
        format!(" (in {})", dir.display())
    };
    let runnable =
        detection.schema.is_some() || detection.dbt.as_ref().is_some_and(|d| d.has_catalog);
    let mut next: Vec<String> = Vec::new();
    if !runnable || detection.queries.is_empty() {
        if detection.schema.is_none() {
            if let Some(dbt) = detection.dbt.as_ref().filter(|d| !d.has_catalog) {
                let at = if dbt.dir.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", dbt.dir)
                };
                next.push(format!(
                    "Run `dbt docs generate`{at}: sqlsift reads the columns of models and \
                     sources from target/catalog.json"
                ));
            } else {
                next.push(format!(
                    "Set `schema` or `schema_dir` in {CONFIG_FILE} to your DDL or migrations"
                ));
            }
        }
        if detection.queries.is_empty() {
            next.push(format!(
                "Set `files` in {CONFIG_FILE} to your query files (e.g. \"queries/**/*.sql\")"
            ));
        }
        next.push(format!("Then run `sqlsift check`{in_dir}"));
    } else if options.no_check {
        next.push(format!("Run `sqlsift check`{in_dir}"));
    } else {
        println!();
        println!("Running a first check...");
        match first_check(dir, &config_path) {
            Ok(summary) => {
                print!("{}", summary.render());
                if summary.errors + summary.warnings > 0 {
                    next.push(format!(
                        "Run `sqlsift check`{in_dir} to see each diagnostic"
                    ));
                    next.push(format!(
                        "A rule that doesn't fit your code? Set its level in [rules] of {CONFIG_FILE} \
                         (e.g. {} = \"warn\"). Please report false positives at \
                         https://github.com/yukikotani231/sqlsift/issues",
                        summary.top_rule().unwrap_or("E0002")
                    ));
                }
                if summary.errors + summary.warnings >= BASELINE_THRESHOLD {
                    next.push(format!(
                        "To adopt sqlsift now and fix these later: `sqlsift check --write-baseline` \
                         records them in sqlsift-baseline.json; add `baseline = \"sqlsift-baseline.json\"` \
                         to {CONFIG_FILE} so only new ones are reported"
                    ));
                }
            }
            Err(message) => {
                println!("The first check could not run:");
                for line in message.lines() {
                    println!("  {line}");
                }
                next.push(format!(
                    "Fix {CONFIG_FILE}, then run `sqlsift check`{in_dir}"
                ));
            }
        }
    }
    next.push(format!(
        "Run it in CI: add `- uses: yukikotani231/sqlsift@main` after actions/checkout \
         (it reads {CONFIG_FILE}); see {DOCS}/integrations/ci.html"
    ));

    println!();
    println!("Next steps:");
    for step in next {
        println!("  - {step}");
    }
    Ok(false)
}

/// Other schema sources listed at most
const MAX_OTHER_SCHEMAS: usize = 3;

/// Diagnostics from which the first check suggests a baseline
const BASELINE_THRESHOLD: usize = 10;

/// `path` with `/` separators and without a leading `./`
fn display(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    text.strip_prefix("./")
        .map(ToString::to_string)
        .unwrap_or(text)
}

/// A TOML basic string
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_array(values: &[String]) -> String {
    let items: Vec<String> = values.iter().map(|v| toml_string(v)).collect();
    format!("[{}]", items.join(", "))
}

/// The dialect to write and the comment explaining where it came from
fn chosen_dialect(detection: &Detection, dialect_override: Option<&str>) -> (String, String) {
    match (dialect_override, &detection.dialect) {
        (Some(d), _) => (d.to_string(), "set with --dialect".to_string()),
        (None, Some(hint)) => (
            hint.dialect.to_string(),
            format!("detected from {}", hint.source),
        ),
        (None, None) => (
            "postgresql".to_string(),
            "the default; nothing in the project named a database".to_string(),
        ),
    }
}

/// The `sqlsift.toml` for a detected project, with comments
pub fn render(detection: &Detection, dialect_override: Option<&str>) -> String {
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    line("# sqlsift configuration, written by `sqlsift init`");
    line(&format!("# Every key: {DOCS}/reference/config.html"));
    line("");

    let (dialect, origin) = chosen_dialect(detection, dialect_override);
    line("# SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks");
    line(&format!("# ({origin})"));
    line(&format!("dialect = {}", toml_string(&dialect)));
    line("");

    if let Some(schema) = &detection.schema {
        line(&format!("# Schema: {}", schema.label));
        if schema.schema_dir.is_some() {
            line("# Every .sql file below schema_dir is applied in file name order; rollback");
            line(
                "# migrations (*.down.sql, Flyway U*__, dbmate / goose down sections) are skipped.",
            );
        }
        if let Some(dir) = &schema.schema_dir {
            line(&format!("schema_dir = {}", toml_string(dir)));
        }
        if !schema.schema.is_empty() {
            line(&format!("schema = {}", toml_array(&schema.schema)));
        }
        if !detection.other_schemas.is_empty() {
            line("# Also found (use one of these instead if it is a better source):");
            for other in detection.other_schemas.iter().take(MAX_OTHER_SCHEMAS) {
                if let Some(dir) = &other.schema_dir {
                    line(&format!(
                        "# schema_dir = {}  # {}",
                        toml_string(dir),
                        other.label
                    ));
                }
                if !other.schema.is_empty() {
                    line(&format!(
                        "# schema = {}  # {}",
                        toml_array(&other.schema),
                        other.label
                    ));
                }
            }
        }
    } else {
        match &detection.dbt {
            Some(dbt) if dbt.has_catalog => {
                line("# Schema: the columns of dbt models and sources are read from the dbt");
                line("# project's target/catalog.json (`dbt docs generate`). Add `schema` for");
                line("# tables that are not in it.");
            }
            Some(_) => {
                line("# Schema: run `dbt docs generate` to write target/catalog.json, from which");
                line("# sqlsift reads the columns of dbt models and sources.");
            }
            None => {
                line("# Schema: not found. Point sqlsift at your DDL or migrations:");
            }
        }
        line("# schema = [\"db/schema.sql\"]");
        line("# schema_dir = \"db/migrations\"");
    }
    for note in &detection.notes {
        line(&format!("# Note: {note}"));
    }
    line("");

    if detection.queries.is_empty() {
        line("# Query files to check: none found");
        line("# files = [\"queries/**/*.sql\", \"src/**/*.ts\"]");
    } else {
        line("# Query files to check");
        line("files = [");
        let width = detection
            .queries
            .iter()
            .flat_map(|g| &g.patterns)
            .map(|p| toml_string(p).len() + 1)
            .max()
            .unwrap_or(0);
        for group in &detection.queries {
            for (i, pattern) in group.patterns.iter().enumerate() {
                if i == 0 {
                    line(&format!(
                        "    {:<width$}  # {} ({} file{})",
                        format!("{},", toml_string(pattern)),
                        group.label,
                        group.count,
                        if group.count == 1 { "" } else { "s" }
                    ));
                } else {
                    line(&format!("    {},", toml_string(pattern)));
                }
            }
        }
        line("]");
    }
    if detection.dbt.is_some() {
        line("# dbt models are Jinja templates: `{{ ref() }}` and `{{ source() }}` are");
        line("# resolved, other tags are masked (templating is detected from dbt_project.yml)");
    }
    line("");

    if detection.ignore.is_empty() {
        line("# Query files to skip");
        line("# ignore = [\"queries/archive/**\"]");
    } else {
        line("# Schema files next to the query files, not to be checked as queries");
        line(&format!("ignore = {}", toml_array(&detection.ignore)));
    }
    line("");

    if let Some(tags) = &detection.embedded_sql_tags {
        line("# Tagged templates checked as SQL in TypeScript / JavaScript (Prisma $queryRaw)");
        line(&format!("embedded_sql_tags = {}", toml_array(tags)));
        line("");
    }

    line("# Hide known diagnostics while you fix them: run `sqlsift check --write-baseline`");
    line("# baseline = \"sqlsift-baseline.json\"");
    line("");
    line("# Rule levels: \"off\", \"warn\" or \"error\" (`sqlsift rules` lists every rule)");
    line("[rules]");
    line("# ambiguous-column = \"warn\"");
    out
}

/// What was detected, for the terminal
fn report(detection: &Detection, dialect_override: Option<&str>) -> String {
    let mut out = String::from("Detected:\n");
    let mut row = |name: &str, text: &str| {
        let _ = writeln!(out, "  {name:<9} {text}");
    };
    match &detection.schema {
        Some(schema) => row(
            "schema",
            &format!(
                "{}: {} ({} SQL file{})",
                schema.label,
                schema.paths(),
                schema.sql_files,
                if schema.sql_files == 1 { "" } else { "s" }
            ),
        ),
        None => match &detection.dbt {
            Some(dbt) if dbt.has_catalog => row("schema", "dbt catalog (target/catalog.json)"),
            Some(_) => row(
                "schema",
                "none yet (dbt project without target/catalog.json)",
            ),
            None => row("schema", "not found"),
        },
    }
    for other in detection.other_schemas.iter().take(MAX_OTHER_SCHEMAS) {
        row("", &format!("also: {}: {}", other.label, other.paths()));
    }
    if detection.other_schemas.len() > MAX_OTHER_SCHEMAS {
        let more = detection.other_schemas.len() - MAX_OTHER_SCHEMAS;
        row("", &format!("and {more} more"));
    }
    let (dialect, origin) = chosen_dialect(detection, dialect_override);
    row("dialect", &format!("{dialect} ({origin})"));
    if dialect_override.is_none() {
        for other in &detection.other_dialects {
            row(
                "",
                &format!("but {} suggests {}", other.source, other.dialect),
            );
        }
    }
    if detection.queries.is_empty() {
        row("queries", "not found");
    }
    for (i, group) in detection.queries.iter().enumerate() {
        row(
            if i == 0 { "queries" } else { "" },
            &format!(
                "{}: {} ({} file{})",
                group.label,
                group.patterns.join(", "),
                group.count,
                if group.count == 1 { "" } else { "s" }
            ),
        );
    }
    for note in &detection.notes {
        row("note", note);
    }
    out
}

/// The outcome of the first check
#[derive(Debug, Default)]
struct CheckSummary {
    files: usize,
    errors: usize,
    warnings: usize,
    /// Diagnostics per rule code
    rules: BTreeMap<String, usize>,
    /// Diagnostics per file
    files_with_diagnostics: BTreeMap<String, usize>,
    /// Warnings printed while loading the schema
    schema_warnings: Option<String>,
}

impl CheckSummary {
    /// The rule with the most diagnostics
    fn top_rule(&self) -> Option<&str> {
        self.rules
            .iter()
            .max_by_key(|(code, n)| (**n, std::cmp::Reverse((*code).clone())))
            .map(|(code, _)| code.as_str())
    }

    fn render(&self) -> String {
        let mut out = String::new();
        if let Some(warnings) = &self.schema_warnings {
            let _ = writeln!(
                out,
                "  {warnings} (run `sqlsift schema` to see what was loaded)"
            );
        }
        if self.errors + self.warnings == 0 {
            let _ = writeln!(out, "  All {} file(s) passed", self.files);
            return out;
        }
        let _ = writeln!(
            out,
            "  {} error(s), {} warning(s) in {} of {} file(s)",
            self.errors,
            self.warnings,
            self.files_with_diagnostics.len(),
            self.files
        );
        let mut rules: Vec<(&String, &usize)> = self.rules.iter().collect();
        rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (code, count) in rules.iter().take(5) {
            let name = sqlsift_core::rules::find_rule(code).map_or("", |r| r.name);
            let _ = writeln!(out, "    {code} {name:<26} {count}");
        }
        if rules.len() > 5 {
            let _ = writeln!(out, "    ... and {} more rule(s)", rules.len() - 5);
        }
        out
    }
}

/// Run `sqlsift check` with the new configuration and summarize its JSON output.
/// Returns the error output when the check could not run.
fn first_check(dir: &Path, config: &Path) -> std::result::Result<CheckSummary, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let config = config
        .canonicalize()
        .unwrap_or_else(|_| config.to_path_buf());
    let output = std::process::Command::new(exe)
        .current_dir(dir)
        .arg("check")
        .arg("--config")
        .arg(&config)
        .args(["--format", "json", "--max-errors", "0"])
        .output()
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.code() == Some(2) || output.status.code().is_none() {
        let message = stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(10)
            .collect::<Vec<_>>()
            .join("\n");
        return Err(message);
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("{e}\n{stderr}"))?;
    let mut summary = CheckSummary {
        schema_warnings: stderr
            .lines()
            .find(|l| l.starts_with("Warning: Schema parsing produced"))
            .map(|l| {
                l.trim_start_matches("Warning: ")
                    .trim_end_matches(':')
                    .to_string()
            }),
        ..CheckSummary::default()
    };
    let files = json
        .get("files")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    // The JSON output lists only files with diagnostics; the number of files
    // checked is in the summary line on stderr
    summary.files = checked_file_count(&stderr).unwrap_or(files.len());
    for file in &files {
        let name = file
            .get("file")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let diagnostics = file
            .get("diagnostics")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for diagnostic in &diagnostics {
            match diagnostic
                .get("severity")
                .and_then(serde_json::Value::as_str)
            {
                Some("error") => summary.errors += 1,
                Some("warning") => summary.warnings += 1,
                _ => continue,
            }
            let code = diagnostic
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            *summary.rules.entry(code.to_string()).or_default() += 1;
            *summary
                .files_with_diagnostics
                .entry(name.to_string())
                .or_default() += 1;
        }
    }
    Ok(summary)
}

/// The number of files in the summary line of `sqlsift check` ("All 3 file(s)
/// passed validation", "Found 1 error(s), 0 warning(s) in 3 file(s)")
fn checked_file_count(stderr: &str) -> Option<usize> {
    stderr.lines().rev().find_map(|line| {
        let before = line
            .strip_suffix(" file(s) passed validation")
            .or_else(|| {
                line.starts_with("Found ")
                    .then(|| line.strip_suffix(" file(s)"))
                    .flatten()
            })?;
        before.rsplit(' ').next()?.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_checked_file_count() {
        assert_eq!(
            checked_file_count("All 3 file(s) passed validation\n"),
            Some(3)
        );
        assert_eq!(
            checked_file_count("\nFound 1 error(s), 0 warning(s) in 12 file(s)\n"),
            Some(12)
        );
        assert_eq!(checked_file_count("nothing"), None);
    }

    #[test]
    fn escapes_toml_strings() {
        assert_eq!(toml_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(
            toml_array(&["x".to_string(), "y".to_string()]),
            "[\"x\", \"y\"]"
        );
    }

    #[test]
    fn empty_detection_renders_valid_toml() {
        let config = render(&Detection::default(), None);
        let table: toml::Table = toml::from_str(&config).unwrap();
        assert_eq!(table["dialect"].as_str(), Some("postgresql"));
        assert!(table.get("files").is_none());
    }
}
