//! End-to-end coverage tests for the `sqlsift` CLI binary.
//!
//! Every test runs the compiled binary (`CARGO_BIN_EXE_sqlsift`) inside its own
//! temporary directory so that tests are hermetic and can run in parallel.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const USERS_SCHEMA: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n";
const ORDERS_SCHEMA: &str =
    "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, total NUMERIC);\n";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX_EPOCH")
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "sqlsift-cov-{prefix}-{}-{n}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp dir");
        let path = path.canonicalize().expect("canonicalize temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Write a file relative to the temp dir (creating parent directories).
    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        fs::write(&p, content).expect("failed to write file");
        p
    }

    fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.path.join(rel);
        fs::create_dir_all(&p).expect("failed to create dir");
        p
    }

    /// Run `sqlsift` with the temp dir as the working directory.
    fn run(&self, args: &[&str]) -> Run {
        self.run_in(&self.path, args)
    }

    // A method like `run`, though it runs in `cwd` instead of the project directory
    #[allow(clippy::unused_self)]
    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_sqlsift"))
            .current_dir(cwd)
            .args(args)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .output()
            .expect("failed to execute sqlsift");
        Run::from(output)
    }

    /// Run `sqlsift` with the temp dir as the working directory, writing `input` to stdin.
    fn run_stdin(&self, args: &[&str], input: &str) -> Run {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(env!("CARGO_BIN_EXE_sqlsift"))
            .current_dir(&self.path)
            .args(args)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to execute sqlsift");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // sqlsift may exit without reading stdin (e.g. on a usage error)
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
        Run::from(
            child
                .wait_with_output()
                .expect("failed to wait for sqlsift"),
        )
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Captured process result with convenience accessors.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    raw_stderr: String,
}

impl From<Output> for Run {
    fn from(o: Output) -> Self {
        Self {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: strip_ansi(&String::from_utf8_lossy(&o.stderr)),
            raw_stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }
}

impl Run {
    fn assert_code(&self, expected: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(expected),
            "unexpected exit code\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    fn assert_stderr_contains(&self, needle: &str) -> &Self {
        // miette wraps long messages (e.g. long temp paths on macOS) and draws a
        // `│` gutter, and Windows paths use `\`, so compare with whitespace,
        // gutter characters and path separators normalized
        let normalize = |s: &str| {
            s.replace('│', " ")
                .replace('\\', "/")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(
            self.stderr.contains(needle) || normalize(&self.stderr).contains(&normalize(needle)),
            "expected stderr to contain {needle:?}\n--- stderr ---\n{}",
            self.stderr
        );
        self
    }

    fn assert_stderr_lacks(&self, needle: &str) -> &Self {
        assert!(
            !self.stderr.contains(needle),
            "expected stderr NOT to contain {needle:?}\n--- stderr ---\n{}",
            self.stderr
        );
        self
    }

    fn assert_stdout_contains(&self, needle: &str) -> &Self {
        assert!(
            self.stdout.contains(needle),
            "expected stdout to contain {needle:?}\n--- stdout ---\n{}",
            self.stdout
        );
        self
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is not valid JSON ({e})\n--- stdout ---\n{}",
                self.stdout
            )
        })
    }

    /// Count `error[CODE]` headers in human output.
    fn count_code(&self, code: &str) -> usize {
        self.stderr.matches(&format!("error[{code}]")).count()
    }
}

/// Remove ANSI CSI escape sequences (`ESC [ ... letter`).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Temp dir pre-populated with `schema.sql` (users table).
fn with_users_schema(prefix: &str) -> TempDir {
    let t = TempDir::new(prefix);
    t.write("schema.sql", USERS_SCHEMA);
    t
}

/// Returns true when no ancestor of the temp root contains a `sqlsift.toml`
/// (otherwise "no config" scenarios would be polluted by auto-discovery).
fn temp_root_is_config_free(t: &TempDir) -> bool {
    t.path()
        .ancestors()
        .skip(1)
        .all(|a| !a.join("sqlsift.toml").exists())
}

fn diag_codes(json: &Value) -> Vec<String> {
    json["files"]
        .as_array()
        .expect("files array")
        .iter()
        .flat_map(|f| {
            f["diagnostics"]
                .as_array()
                .expect("diagnostics array")
                .clone()
        })
        .map(|d| d["kind"].as_str().expect("kind string").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Exit codes & file handling
// ---------------------------------------------------------------------------

#[test]
fn valid_query_exits_zero_with_pass_summary() {
    let t = with_users_schema("valid");
    t.write("q.sql", "SELECT id, name FROM users;\n");
    t.run(&["check", "--schema", "schema.sql", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn invalid_query_exits_one_with_error_summary() {
    let t = with_users_schema("invalid");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "--schema", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Found 1 error(s), 0 warning(s) in 1 file(s)");
}

#[test]
fn tables_created_in_a_query_file_are_local_to_that_file() {
    let t = with_users_schema("file-local-tables");
    t.write(
        "a.sql",
        "CREATE TEMP TABLE tmp_names AS SELECT id, name FROM users;\n\
         SELECT name FROM tmp_names;\n\
         SELECT nmae FROM tmp_names;\n\
         DROP TABLE tmp_names;\n",
    );
    t.write("b.sql", "SELECT name FROM tmp_names;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "a.sql", "b.sql"]);
    run.assert_code(1);
    let v = run.json();
    let files = v["files"].as_array().expect("files array");
    let codes_of = |name: &str| -> Vec<String> {
        files
            .iter()
            .filter(|f| f["file"] == name)
            .flat_map(|f| f["diagnostics"].as_array().expect("diagnostics").clone())
            .map(|d| d["kind"].as_str().expect("kind").to_string())
            .collect()
    };
    // The typo against the temp table is reported, the table itself is found
    assert_eq!(codes_of("a.sql"), vec!["ColumnNotFound"]);
    // The temp table isn't visible in other files
    assert_eq!(codes_of("b.sql"), vec!["TableNotFound"]);
}

#[test]
fn short_schema_flag_is_accepted() {
    let t = with_users_schema("short-s");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn missing_schema_file_exits_two() {
    let t = TempDir::new("missing-schema");
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "--schema", "does_not_exist.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Error");
}

#[test]
fn no_schema_specified_exits_two_with_hint() {
    let t = TempDir::new("no-schema");
    if !temp_root_is_config_free(&t) {
        return;
    }
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("No schema files specified");
}

#[test]
fn nonexistent_query_file_exits_two() {
    let t = with_users_schema("missing-query");
    t.run(&["check", "--schema", "schema.sql", "nope.sql"])
        .assert_code(2)
        .assert_stderr_contains("Error");
}

#[test]
fn directory_as_query_file_exits_two() {
    let t = with_users_schema("dir-query");
    t.mkdir("subdir");
    t.run(&["check", "--schema", "schema.sql", "subdir"])
        .assert_code(2);
}

#[test]
fn no_query_files_exits_two_with_hint() {
    let t = with_users_schema("no-query");
    if !temp_root_is_config_free(&t) {
        return;
    }
    t.run(&["check", "--schema", "schema.sql"])
        .assert_code(2)
        .assert_stderr_contains("No query files specified");
}

#[test]
fn empty_query_file_passes() {
    let t = with_users_schema("empty-query");
    t.write("q.sql", "");
    t.run(&["check", "--schema", "schema.sql", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn comment_only_query_file_passes() {
    let t = with_users_schema("comment-query");
    t.write("q.sql", "-- just a comment\n/* block */\n\n");
    t.run(&["check", "--schema", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn glob_matching_nothing_exits_two() {
    let t = with_users_schema("glob-none");
    t.run(&["check", "--schema", "schema.sql", "queries/*.sql"])
        .assert_code(2)
        .assert_stderr_contains("No files match 'queries/*.sql'")
        .assert_stderr_lacks("No query files specified");
}

#[test]
fn glob_matching_nothing_names_every_pattern() {
    let t = with_users_schema("glob-none-multi");
    t.run(&["check", "-s", "schema.sql", "a/*.sql", "b/**/*.sql"])
        .assert_code(2)
        .assert_stderr_contains("No files match 'a/*.sql', 'b/**/*.sql'");
}

#[test]
fn config_files_glob_matching_nothing_names_the_pattern() {
    let t = with_users_schema("glob-none-cfg");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"sql/none/*.sql\"]\n",
    );
    t.run(&["check"])
        .assert_code(2)
        .assert_stderr_contains("No files match 'sql/none/*.sql'");
}

#[test]
fn glob_matching_nothing_warns_when_other_patterns_match() {
    let t = with_users_schema("glob-some");
    t.write("queries/a.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "queries/*.sql", "typo/*.sql"])
        .assert_code(0)
        .assert_stderr_contains("Warning: no files match 'typo/*.sql'")
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn glob_star_matches_multiple_files() {
    let t = with_users_schema("glob-star");
    t.write("queries/a.sql", "SELECT id FROM users;\n");
    t.write("queries/b.sql", "SELECT name FROM users;\n");
    t.write("queries/c.txt", "SELECT broken FROM nowhere;\n");
    t.run(&["check", "--schema", "schema.sql", "queries/*.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 2 file(s) passed validation");
}

#[test]
fn glob_double_star_recurses_into_subdirectories() {
    let t = with_users_schema("glob-recursive");
    t.write("queries/top.sql", "SELECT id FROM users;\n");
    t.write("queries/nested/mid.sql", "SELECT name FROM users;\n");
    t.write("queries/nested/deep/bad.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "--schema", "schema.sql", "queries/**/*.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("bad.sql:1:8")
        .assert_stderr_contains("in 3 file(s)");
}

#[test]
fn multiple_explicit_files_errors_in_second_only() {
    let t = with_users_schema("multi");
    t.write("good.sql", "SELECT id FROM users;\n");
    t.write("bad.sql", "SELECT id FROM nope;\n");
    t.run(&["check", "--schema", "schema.sql", "good.sql", "bad.sql"])
        .assert_code(1)
        .assert_stderr_contains("bad.sql:1:16")
        .assert_stderr_lacks("good.sql:")
        .assert_stderr_contains("in 2 file(s)");
}

#[test]
fn multiple_files_all_valid_counts_files() {
    let t = with_users_schema("multi-valid");
    for i in 0..4 {
        t.write(&format!("q{i}.sql"), "SELECT id FROM users;\n");
    }
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "q0.sql",
        "q1.sql",
        "q2.sql",
        "q3.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains("All 4 file(s) passed validation");
}

#[test]
fn errors_summed_across_files() {
    let t = with_users_schema("sum");
    t.write("a.sql", "SELECT a1 FROM users;\nSELECT a2 FROM users;\n");
    t.write("b.sql", "SELECT b1 FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "a.sql", "b.sql"])
        .assert_code(1)
        .assert_stderr_contains("Found 3 error(s), 0 warning(s) in 2 file(s)");
}

#[test]
fn query_parse_error_reports_e1000() {
    let t = with_users_schema("parse-err");
    t.write("q.sql", "SELEC id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E1000]")
        .assert_stderr_contains("Parse error");
}

// ---------------------------------------------------------------------------
// --max-errors, --quiet, --verbose
// ---------------------------------------------------------------------------

fn many_errors(n: usize) -> String {
    let mut sql = String::new();
    for i in 0..n {
        let _ = writeln!(sql, "SELECT missing_{i} FROM users;");
    }
    sql
}

#[test]
fn default_max_errors_is_100() {
    let t = with_users_schema("max-default");
    t.write("q.sql", &many_errors(105));
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Reached maximum error limit (100). Stopped early.")
        .assert_stderr_contains("Found 100 error(s)");
    assert_eq!(run.count_code("E0002"), 100);
}

#[test]
fn max_errors_zero_means_unlimited() {
    let t = with_users_schema("max-zero");
    t.write("q.sql", &many_errors(105));
    let run = t.run(&["check", "-s", "schema.sql", "--max-errors", "0", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_lacks("Reached maximum error limit")
        .assert_stderr_contains("Found 105 error(s)");
    assert_eq!(run.count_code("E0002"), 105);
}

#[test]
fn max_errors_above_count_has_no_limit_message() {
    let t = with_users_schema("max-above");
    t.write("q.sql", &many_errors(3));
    t.run(&["check", "-s", "schema.sql", "--max-errors", "10", "q.sql"])
        .assert_code(1)
        .assert_stderr_lacks("Reached maximum error limit")
        .assert_stderr_contains("Found 3 error(s)");
}

#[test]
fn max_errors_stops_before_later_files() {
    let t = with_users_schema("max-files");
    t.write(
        "a.sql",
        "SELECT first_missing FROM users;\nSELECT second_missing FROM users;\n",
    );
    t.write("b.sql", "SELECT third_missing FROM users;\n");
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--max-errors",
        "2",
        "a.sql",
        "b.sql",
    ]);
    run.assert_code(1)
        .assert_stderr_contains("first_missing")
        .assert_stderr_contains("second_missing")
        .assert_stderr_lacks("third_missing")
        .assert_stderr_contains("Reached maximum error limit (2)");
}

#[test]
fn max_errors_limits_json_output_too() {
    let t = with_users_schema("max-json");
    t.write("q.sql", &many_errors(5));
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--max-errors",
        "2",
        "-f",
        "json",
        "q.sql",
    ]);
    run.assert_code(1);
    assert_eq!(
        run.json()["files"][0]["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn max_errors_rejects_non_numeric() {
    let t = with_users_schema("max-nan");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--max-errors", "lots", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("invalid value");
}

#[test]
fn quiet_valid_run_prints_nothing() {
    let t = with_users_schema("quiet-valid");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["-q", "check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(0);
    assert!(run.stderr.trim().is_empty(), "stderr: {}", run.stderr);
    assert!(run.stdout.trim().is_empty(), "stdout: {}", run.stdout);
}

#[test]
fn quiet_suppresses_max_errors_message_but_keeps_exit_code() {
    let t = with_users_schema("quiet-max");
    t.write("q.sql", &many_errors(3));
    t.run(&[
        "--quiet",
        "check",
        "-s",
        "schema.sql",
        "--max-errors",
        "1",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_lacks("Reached maximum error limit")
    .assert_stderr_contains("error[E0002]");
}

#[test]
fn quiet_flag_after_subcommand_is_accepted() {
    let t = with_users_schema("quiet-global");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["check", "-q", "-s", "schema.sql", "q.sql"]);
    run.assert_code(0);
    assert!(run.stderr.trim().is_empty(), "stderr: {}", run.stderr);
}

#[test]
fn quiet_json_still_emits_json() {
    let t = with_users_schema("quiet-json");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["-q", "check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    assert_eq!(diag_codes(&run.json()), vec!["ColumnNotFound"]);
    assert!(run.stderr.trim().is_empty(), "stderr: {}", run.stderr);
}

#[test]
fn double_verbose_emits_debug_logs() {
    let t = with_users_schema("vv");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["-vv", "check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(0);
    let all = strip_ansi(&format!("{}{}", run.stdout, run.stderr));
    assert!(all.contains("Analyzing SQL file"), "output:\n{all}");
}

#[test]
fn verbose_does_not_change_exit_code_on_errors() {
    let t = with_users_schema("v-err");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["-v", "check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0002]");
}

// ---------------------------------------------------------------------------
// Top-level CLI surface
// ---------------------------------------------------------------------------

#[test]
fn no_subcommand_prints_usage_and_exits_two() {
    let t = TempDir::new("nosub");
    t.run(&[]).assert_code(2).assert_stderr_contains("Usage");
}

#[test]
fn version_flag_prints_crate_version() {
    let t = TempDir::new("version");
    t.run(&["--version"])
        .assert_code(0)
        .assert_stdout_contains(env!("CARGO_PKG_VERSION"));
}

#[test]
fn check_help_lists_options() {
    let t = TempDir::new("help");
    let run = t.run(&["check", "--help"]);
    run.assert_code(0);
    for opt in [
        "--schema",
        "--schema-dir",
        "--config",
        "--disable",
        "--dialect",
        "--format",
        "--max-errors",
    ] {
        run.assert_stdout_contains(opt);
    }
}

#[test]
fn invalid_format_value_is_rejected_by_clap() {
    let t = with_users_schema("bad-format");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--format", "xml", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("possible values: human, json, sarif");
}

#[test]
fn unknown_flag_is_rejected() {
    let t = with_users_schema("bad-flag");
    t.run(&["check", "--frobnicate", "q.sql"]).assert_code(2);
}

#[test]
fn schema_subcommand_prints_tables_and_columns() {
    let t = TempDir::new("schema-cmd");
    t.write("a.sql", USERS_SCHEMA);
    t.write("b.sql", ORDERS_SCHEMA);
    t.run(&["schema", "a.sql", "b.sql"])
        .assert_code(0)
        .assert_stdout_contains("Table: users")
        .assert_stdout_contains("Table: orders")
        .assert_stdout_contains("- name text NOT NULL")
        .assert_stdout_contains("- total");
}

#[test]
fn schema_subcommand_requires_files() {
    let t = TempDir::new("schema-cmd-none");
    let run = t.run(&["schema"]);
    run.assert_code(2);
    if temp_root_is_config_free(&t) {
        run.assert_stderr_contains("No schema files specified");
    }
}

const RICH_SCHEMA: &str = "\
CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, feeling mood);
CREATE VIEW user_names AS SELECT id, name FROM users;
CREATE MATERIALIZED VIEW user_count AS SELECT COUNT(*) AS n FROM users;
CREATE TABLE audit.events (id BIGINT NOT NULL, user_id INTEGER REFERENCES users (id));
";

#[test]
fn schema_subcommand_accepts_schema_flag() {
    let t = TempDir::new("schema-cmd-flag");
    t.write("a.sql", USERS_SCHEMA);
    t.write("b.sql", ORDERS_SCHEMA);
    t.run(&["schema", "-s", "a.sql", "--schema", "b.sql"])
        .assert_code(0)
        .assert_stdout_contains("Table: users")
        .assert_stdout_contains("Table: orders");
}

#[test]
fn schema_subcommand_accepts_schema_dir() {
    let t = TempDir::new("schema-cmd-dir");
    t.write("migrations/001_users.sql", USERS_SCHEMA);
    t.write("migrations/002_orders.sql", ORDERS_SCHEMA);
    t.write(
        "migrations/003_alter.sql",
        "ALTER TABLE users ADD COLUMN email TEXT;\n",
    );
    let run = t.run(&["schema", "--schema-dir", "migrations"]);
    run.assert_code(0)
        .assert_stdout_contains("Table: users")
        .assert_stdout_contains("Table: orders")
        .assert_stdout_contains("- email text");
}

#[test]
fn schema_subcommand_missing_schema_dir_exits_two() {
    let t = TempDir::new("schema-cmd-dir-missing");
    t.run(&["schema", "--schema-dir", "nope"])
        .assert_code(2)
        .assert_stderr_contains("Schema directory not found");
}

#[test]
fn schema_subcommand_falls_back_to_config() {
    let t = TempDir::new("schema-cmd-config");
    t.write("db/migrations/001.sql", USERS_SCHEMA);
    t.write("sqlsift.toml", "schema_dir = \"db/migrations\"\n");
    t.run(&["schema"])
        .assert_code(0)
        .assert_stdout_contains("Table: users");

    // From a subdirectory, the config is discovered and paths stay relative to it
    let sub = t.mkdir("src/queries");
    t.run_in(&sub, &["schema"])
        .assert_code(0)
        .assert_stdout_contains("Table: users");
}

#[test]
fn schema_subcommand_explicit_config_path() {
    let t = TempDir::new("schema-cmd-config-path");
    t.write("conf/schema.sql", ORDERS_SCHEMA);
    t.write(
        "conf/custom.toml",
        "schema = [\"schema.sql\"]\ndialect = \"postgresql\"\n",
    );
    t.run(&["schema", "-c", "conf/custom.toml"])
        .assert_code(0)
        .assert_stdout_contains("Table: orders");
}

#[test]
fn schema_subcommand_cli_overrides_config() {
    let t = TempDir::new("schema-cmd-override");
    t.write("a.sql", USERS_SCHEMA);
    t.write("b.sql", ORDERS_SCHEMA);
    t.write("sqlsift.toml", "schema = [\"a.sql\"]\n");
    let run = t.run(&["schema", "b.sql"]);
    run.assert_code(0).assert_stdout_contains("Table: orders");
    assert!(
        !run.stdout.contains("Table: users"),
        "positional files should replace config schema\n{}",
        run.stdout
    );
}

#[test]
fn schema_subcommand_respects_dialect() {
    let t = TempDir::new("schema-cmd-dialect");
    t.write(
        "schema.sql",
        "CREATE TABLE `items` (`id` INT NOT NULL AUTO_INCREMENT, `kind` ENUM('a', 'b'), PRIMARY KEY (`id`));\n",
    );
    t.run(&["schema", "-d", "mysql", "-s", "schema.sql"])
        .assert_code(0)
        .assert_stdout_contains("Table: items")
        .assert_stdout_contains("- kind enum('a', 'b')");
    t.run(&["schema", "-d", "oracle", "-s", "schema.sql"])
        .assert_code(2);
}

#[test]
fn schema_subcommand_shows_views_enums_and_schemas() {
    let t = TempDir::new("schema-cmd-rich");
    t.write("schema.sql", RICH_SCHEMA);
    let run = t.run(&["schema", "-s", "schema.sql"]);
    run.assert_code(0)
        .assert_stdout_contains("Schema: public")
        .assert_stdout_contains("Schema: audit")
        .assert_stdout_contains("Table: users")
        .assert_stdout_contains("- id integer NOT NULL PRIMARY KEY")
        .assert_stdout_contains("- feeling mood NULL")
        .assert_stdout_contains("View: user_names")
        .assert_stdout_contains("Materialized view: user_count")
        .assert_stdout_contains("- n bigint")
        .assert_stdout_contains("Table: events")
        .assert_stdout_contains("Enum types:")
        .assert_stdout_contains("mood: 'sad', 'ok', 'happy'");

    // Output is deterministic and follows definition order
    let again = t.run(&["schema", "-s", "schema.sql"]);
    assert_eq!(run.stdout, again.stdout);
    let users = run.stdout.find("Table: users").unwrap();
    let view = run.stdout.find("View: user_names").unwrap();
    assert!(
        users < view,
        "tables are listed before views\n{}",
        run.stdout
    );
}

#[test]
fn schema_subcommand_json_output() {
    let t = TempDir::new("schema-cmd-json");
    t.write("schema.sql", RICH_SCHEMA);
    let run = t.run(&["schema", "--format", "json", "-s", "schema.sql"]);
    run.assert_code(0);
    let json = run.json();

    assert_eq!(json["dialect"], "postgresql");
    assert_eq!(json["default_schema"], "public");
    assert_eq!(json["schema_files"][0], "schema.sql");

    let schemas = json["schemas"].as_array().expect("schemas array");
    let names: Vec<_> = schemas
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["public", "audit"]);

    let public = &schemas[0];
    let users = &public["tables"][0];
    assert_eq!(users["name"], "users");
    assert_eq!(users["primary_key"], serde_json::json!(["id"]));
    assert_eq!(users["columns"][0]["name"], "id");
    assert_eq!(users["columns"][0]["type"], "integer");
    assert_eq!(users["columns"][0]["nullable"], false);
    assert_eq!(users["columns"][0]["primary_key"], true);
    assert_eq!(users["columns"][2]["type"], "mood");
    assert_eq!(users["columns"][2]["nullable"], true);

    let views = public["views"].as_array().expect("views array");
    assert_eq!(views[0]["name"], "user_names");
    assert_eq!(views[0]["materialized"], false);
    assert_eq!(views[0]["columns"][1]["name"], "name");
    assert_eq!(views[0]["columns"][1]["type"], "text");
    assert_eq!(views[1]["name"], "user_count");
    assert_eq!(views[1]["materialized"], true);

    let events = &schemas[1]["tables"][0];
    assert_eq!(events["name"], "events");
    assert_eq!(
        events["foreign_keys"][0]["columns"],
        serde_json::json!(["user_id"])
    );
    assert_eq!(events["foreign_keys"][0]["references_table"], "users");

    assert_eq!(json["enums"][0]["name"], "mood");
    assert_eq!(
        json["enums"][0]["values"],
        serde_json::json!(["sad", "ok", "happy"])
    );
}

#[test]
fn schema_subcommand_rejects_sarif_format() {
    let t = with_users_schema("schema-cmd-sarif");
    t.run(&["schema", "--format", "sarif", "-s", "schema.sql"])
        .assert_code(2)
        .assert_stderr_contains("possible values: human, json");
}

#[test]
fn schema_subcommand_missing_file_exits_two() {
    let t = TempDir::new("schema-cmd-missing");
    t.run(&["schema", "nope.sql"]).assert_code(2);
}

#[test]
fn parse_subcommand_prints_statements() {
    let t = TempDir::new("parse-cmd");
    t.write("q.sql", "SELECT 1;\nSELECT 2;\n");
    t.run(&["parse", "q.sql"])
        .assert_code(0)
        .assert_stdout_contains("Statement 1:")
        .assert_stdout_contains("Statement 2:");
}

#[test]
fn parse_subcommand_reports_parse_error() {
    let t = TempDir::new("parse-cmd-err");
    t.write("q.sql", "SELEC 1;\n");
    t.run(&["parse", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Parse error");
}

// ---------------------------------------------------------------------------
// Schema loading
// ---------------------------------------------------------------------------

#[test]
fn repeated_schema_flags_are_merged() {
    let t = TempDir::new("schema-repeat");
    t.write("users.sql", USERS_SCHEMA);
    t.write("orders.sql", ORDERS_SCHEMA);
    t.write(
        "q.sql",
        "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id;\n",
    );
    t.run(&["check", "-s", "users.sql", "-s", "orders.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn only_one_schema_flag_misses_other_tables() {
    let t = TempDir::new("schema-one");
    t.write("users.sql", USERS_SCHEMA);
    t.write("orders.sql", ORDERS_SCHEMA);
    t.write("q.sql", "SELECT total FROM orders;\n");
    t.run(&["check", "-s", "users.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0001]");
}

#[test]
fn repeated_schema_flags_apply_alter_in_order() {
    let t = TempDir::new("schema-alter");
    t.write("1.sql", USERS_SCHEMA);
    t.write("2.sql", "ALTER TABLE users ADD COLUMN email TEXT;\n");
    t.write("q.sql", "SELECT email FROM users;\n");
    t.run(&["check", "-s", "1.sql", "-s", "2.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn schema_dir_applies_migrations_in_filename_order() {
    let t = TempDir::new("schema-dir-order");
    t.write("migrations/001_create.sql", USERS_SCHEMA);
    t.write(
        "migrations/002_alter.sql",
        "ALTER TABLE users ADD COLUMN email TEXT;\n",
    );
    t.write("q.sql", "SELECT id, email FROM users;\n");
    t.run(&["check", "--schema-dir", "migrations", "q.sql"])
        .assert_code(0);
}

#[test]
fn schema_dir_later_migration_drops_column() {
    let t = TempDir::new("schema-dir-drop");
    t.write(
        "migrations/001_create.sql",
        "CREATE TABLE users (id INTEGER, legacy TEXT);\n",
    );
    t.write(
        "migrations/002_drop.sql",
        "ALTER TABLE users DROP COLUMN legacy;\n",
    );
    t.write("q.sql", "SELECT legacy FROM users;\n");
    t.run(&["check", "--schema-dir", "migrations", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0002]")
        .assert_stderr_contains("legacy");
}

#[test]
fn schema_dir_rename_column_in_later_migration() {
    let t = TempDir::new("schema-dir-rename");
    t.write(
        "migrations/001_create.sql",
        "CREATE TABLE users (id INTEGER, fullname TEXT);\n",
    );
    t.write(
        "migrations/002_rename.sql",
        "ALTER TABLE users RENAME COLUMN fullname TO display_name;\n",
    );
    t.write("new.sql", "SELECT display_name FROM users;\n");
    t.write("old.sql", "SELECT fullname FROM users;\n");
    t.run(&["check", "--schema-dir", "migrations", "new.sql"])
        .assert_code(0);
    t.run(&["check", "--schema-dir", "migrations", "old.sql"])
        .assert_code(1);
}

#[test]
fn schema_dir_reads_nested_directories() {
    let t = TempDir::new("schema-dir-nested");
    t.write("db/users.sql", USERS_SCHEMA);
    t.write("db/sales/orders/orders.sql", ORDERS_SCHEMA);
    t.write(
        "q.sql",
        "SELECT o.total, u.name FROM orders o JOIN users u ON u.id = o.user_id;\n",
    );
    t.run(&["check", "--schema-dir", "db", "q.sql"])
        .assert_code(0);
}

#[test]
fn schema_dir_ignores_non_sql_files() {
    let t = TempDir::new("schema-dir-nonsql");
    t.write("db/users.sql", USERS_SCHEMA);
    t.write("db/README.md", "CREATE TABLE docs_only (id INTEGER);\n");
    t.write(
        "db/extra.sql.bak",
        "CREATE TABLE backup_only (id INTEGER);\n",
    );
    t.write(
        "q.sql",
        "SELECT id FROM docs_only;\nSELECT id FROM backup_only;\n",
    );
    let run = t.run(&["check", "--schema-dir", "db", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Table 'docs_only' not found")
        .assert_stderr_contains("Table 'backup_only' not found");
}

#[test]
fn schema_dir_with_no_sql_files_exits_two() {
    let t = TempDir::new("schema-dir-empty");
    t.mkdir("empty");
    t.write("q.sql", "SELECT 1;\n");
    if !temp_root_is_config_free(&t) {
        return;
    }
    t.run(&["check", "--schema-dir", "empty", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("No .sql files found in schema directory")
        .assert_stderr_contains("empty");
}

#[test]
fn schema_flag_and_schema_dir_combined() {
    let t = TempDir::new("schema-both");
    t.write("users.sql", USERS_SCHEMA);
    t.write("db/orders.sql", ORDERS_SCHEMA);
    t.write("q.sql", "SELECT u.id, o.total FROM users u, orders o;\n");
    t.run(&["check", "-s", "users.sql", "--schema-dir", "db", "q.sql"])
        .assert_code(0);
}

#[test]
fn schema_with_unsupported_statement_is_resilient() {
    let t = TempDir::new("schema-resilient");
    t.write(
        "schema.sql",
        "CREATE TABLE users (id INTEGER, name TEXT);\n\
         CREATE FUNCTION f() RETURNS trigger AS $$ BEGIN RETURN NEW; END; $$ LANGUAGE plpgsql;\n\
         THIS IS NOT SQL AT ALL;\n\
         CREATE TABLE orders (id INTEGER, user_id INTEGER);\n",
    );
    t.write(
        "q.sql",
        "SELECT u.name, o.id FROM users u JOIN orders o ON o.user_id = u.id;\n",
    );
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn alter_on_unknown_table_produces_schema_warning_only() {
    let t = TempDir::new("schema-warn");
    t.write(
        "schema.sql",
        "CREATE TABLE users (id INTEGER);\nALTER TABLE ghosts ADD COLUMN x INTEGER;\n",
    );
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("Warning: Schema parsing produced 1 warning(s)")
        .assert_stderr_contains("ALTER TABLE references table 'ghosts'");
}

#[test]
fn empty_schema_file_means_tables_not_found() {
    let t = TempDir::new("schema-empty");
    t.write("schema.sql", "");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Table 'users' not found");
}

#[test]
fn schema_with_view_and_enum() {
    let t = TempDir::new("schema-view-enum");
    t.write(
        "schema.sql",
        "CREATE TYPE mood AS ENUM ('happy', 'sad');\n\
         CREATE TABLE people (id INTEGER, feeling mood, name TEXT);\n\
         CREATE VIEW happy_people AS SELECT id, name FROM people WHERE feeling = 'happy';\n",
    );
    t.write("ok.sql", "SELECT id, name FROM happy_people;\n");
    t.write("bad.sql", "SELECT feeling FROM happy_people;\n");
    t.run(&["check", "-s", "schema.sql", "ok.sql"])
        .assert_code(0);
    t.run(&["check", "-s", "schema.sql", "bad.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0002]");
}

// ---------------------------------------------------------------------------
// sqlsift.toml configuration
// ---------------------------------------------------------------------------

#[test]
fn config_auto_discovered_in_cwd() {
    let t = with_users_schema("cfg-cwd");
    t.write("queries/q.sql", "SELECT id FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"queries/*.sql\"]\n",
    );
    t.run(&["check"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn config_auto_discovered_in_parent_directory() {
    let t = with_users_schema("cfg-parent");
    let schema_abs = t.path().join("schema.sql");
    t.write(
        "sqlsift.toml",
        &format!("schema = [{:?}]\n", schema_abs.display().to_string()),
    );
    t.write("sub/deeper/q.sql", "SELECT nme FROM users;\n");
    let cwd = t.path().join("sub/deeper");
    t.run_in(&cwd, &["check", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Column 'nme' not found");
}

#[test]
fn explicit_config_path_is_used() {
    let t = with_users_schema("cfg-explicit");
    let schema_abs = t.path().join("schema.sql");
    t.write(
        "conf/custom.toml",
        &format!(
            "schema = [{:?}]\ndisable = [\"E0002\"]\n",
            schema_abs.display().to_string()
        ),
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "--config", "conf/custom.toml", "q.sql"])
        .assert_code(0);
}

#[test]
fn explicit_config_takes_precedence_over_discovered_config() {
    let t = with_users_schema("cfg-precedence");
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\ndisable = []\n");
    t.write(
        "other.toml",
        "schema = [\"schema.sql\"]\ndisable = [\"E0002\"]\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "q.sql"]).assert_code(1);
    t.run(&["check", "-c", "other.toml", "q.sql"])
        .assert_code(0);
}

#[test]
fn explicit_config_missing_file_exits_two() {
    let t = with_users_schema("cfg-missing");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&[
        "check",
        "--config",
        "nope.toml",
        "-s",
        "schema.sql",
        "q.sql",
    ])
    .assert_code(2);
}

#[test]
fn invalid_toml_reports_parse_error() {
    let t = with_users_schema("cfg-invalid");
    t.write("sqlsift.toml", "schema = [\n");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("TOML parse error")
        .assert_stderr_contains("line 1");
}

#[test]
fn toml_with_wrong_value_type_is_rejected() {
    let t = with_users_schema("cfg-type");
    t.write("sqlsift.toml", "schema = 42\n");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("schema");
}

#[test]
fn toml_unknown_keys_warn_but_continue() {
    let t = with_users_schema("cfg-unknown");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nsome_future_option = true\n[extra]\nfoo = 1\n",
    );
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("unknown key 'some_future_option'")
        .assert_stderr_contains("unknown key 'extra'");
}

#[test]
fn empty_toml_behaves_like_no_config() {
    let t = TempDir::new("cfg-empty");
    if !temp_root_is_config_free(&t) {
        return;
    }
    t.write("sqlsift.toml", "");
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("No schema files specified");
}

#[test]
fn config_disable_filters_rules() {
    let t = with_users_schema("cfg-disable");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ndisable = [\"E0001\", \"E0002\"]\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\nSELECT id FROM nope;\n");
    t.run(&["check", "q.sql"])
        .assert_code(0)
        .assert_stderr_lacks("error[");
}

#[test]
fn config_format_json() {
    let t = with_users_schema("cfg-json");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nformat = \"json\"\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "q.sql"]);
    run.assert_code(1);
    assert_eq!(diag_codes(&run.json()), vec!["ColumnNotFound"]);
}

#[test]
fn config_format_sarif() {
    let t = with_users_schema("cfg-sarif");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nformat = \"sarif\"\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "q.sql"]);
    run.assert_code(1);
    assert_eq!(run.json()["version"], "2.1.0");
}

#[test]
fn config_schema_dir() {
    let t = TempDir::new("cfg-schema-dir");
    t.write("db/001.sql", USERS_SCHEMA);
    t.write("db/002.sql", "ALTER TABLE users ADD COLUMN age INTEGER;\n");
    t.write("sqlsift.toml", "schema_dir = \"db\"\n");
    t.write("q.sql", "SELECT age FROM users;\n");
    t.run(&["check", "q.sql"]).assert_code(0);
}

#[test]
fn config_files_patterns_are_globbed() {
    let t = with_users_schema("cfg-files");
    t.write("sql/a.sql", "SELECT id FROM users;\n");
    t.write("sql/deep/b.sql", "SELECT nme FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"sql/**/*.sql\"]\n",
    );
    t.run(&["check"])
        .assert_code(1)
        .assert_stderr_contains("b.sql:1:8")
        .assert_stderr_contains("in 2 file(s)");
}

#[test]
fn cli_files_override_config_files() {
    let t = with_users_schema("cfg-files-override");
    t.write("bad.sql", "SELECT nme FROM users;\n");
    t.write("good.sql", "SELECT id FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"bad.sql\"]\n",
    );
    t.run(&["check"]).assert_code(1);
    t.run(&["check", "good.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn cli_schema_overrides_config_schema() {
    let t = with_users_schema("cfg-schema-override");
    t.write("sqlsift.toml", "schema = [\"does_not_exist.sql\"]\n");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "q.sql"]).assert_code(2);
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn cli_format_overrides_config_format() {
    let t = with_users_schema("cfg-format-override");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nformat = \"json\"\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-f", "human", "q.sql"]);
    run.assert_code(1).assert_stderr_contains("error[E0002]");
    assert!(run.stdout.trim().is_empty(), "stdout: {}", run.stdout);
}

#[test]
fn cli_rule_flags_are_applied_after_config() {
    let t = with_users_schema("cfg-disable-override");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ndisable = [\"E0002\"]\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\nSELECT id FROM nope;\n");
    // Config alone: only E0001 remains.
    let run = t.run(&["check", "q.sql"]);
    run.assert_code(1);
    assert_eq!(run.count_code("E0002"), 0);
    assert!(run.count_code("E0001") >= 1);
    // CLI --disable adds to the config's settings
    let run = t.run(&["check", "--disable", "E0001", "q.sql"]);
    run.assert_code(0);
    assert_eq!(run.count_code("E0001"), 0);
    assert_eq!(run.count_code("E0002"), 0);
    // ... and --deny turns a rule the config disables back on
    let run = t.run(&["check", "--deny", "E0002", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Column 'nme' not found");
}

#[test]
fn cli_schema_dir_overrides_config_schema_dir() {
    let t = TempDir::new("cfg-schema-dir-override");
    t.write("a/users.sql", "CREATE TABLE users (id INTEGER);\n");
    t.write(
        "b/users.sql",
        "CREATE TABLE users (id INTEGER, email TEXT);\n",
    );
    t.write("sqlsift.toml", "schema_dir = \"a\"\n");
    t.write("q.sql", "SELECT email FROM users;\n");
    t.run(&["check", "q.sql"]).assert_code(1);
    t.run(&["check", "--schema-dir", "b", "q.sql"])
        .assert_code(0);
}

// ---------------------------------------------------------------------------
// --disable and --dialect
// ---------------------------------------------------------------------------

#[test]
fn disable_single_code() {
    let t = with_users_schema("disable-one");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--disable", "E0002", "q.sql"])
        .assert_code(0)
        .assert_stderr_lacks("error[");
}

#[test]
fn disable_multiple_codes() {
    let t = with_users_schema("disable-many");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\nSELECT id FROM nope;\nSELECT id FROM users WHERE id = 'abc';\n",
    );
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--disable",
        "E0001",
        "--disable",
        "E0002",
        "--disable",
        "E0003",
        "q.sql",
    ])
    .assert_code(0);
}

#[test]
fn disable_one_of_two_codes_keeps_the_other() {
    let t = with_users_schema("disable-partial");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'abc';\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "--disable", "E0002", "q.sql"]);
    run.assert_code(1);
    assert_eq!(run.count_code("E0002"), 0);
    assert_eq!(run.count_code("E0003"), 1);
}

#[test]
fn disable_unknown_code_is_an_error() {
    let t = with_users_schema("disable-unknown");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--disable", "E9999", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("unknown rule or category 'E9999'");
}

#[test]
fn misspelled_rule_flag_suggests_the_rule() {
    let t = with_users_schema("allow-typo");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-A",
        "ambigous-column",
        "q.sql",
    ])
    .assert_code(2)
    .assert_stderr_contains("--allow: unknown rule or category 'ambigous-column'")
    .assert_stderr_contains("Did you mean 'ambiguous-column'?");
    t.run(&["check", "-s", "schema.sql", "-W", "suspicous", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Did you mean 'suspicious'?");
}

#[test]
fn misspelled_rule_names_in_config_suggest_the_rule() {
    let t = with_users_schema("cfg-typo");
    t.write("q.sql", "SELECT id FROM users;\n");
    for (config, expected) in [
        (
            "[rules]\ncolumn-not-fond = \"off\"\n",
            "[rules]: unknown rule 'column-not-fond'. Did you mean 'column-not-found'?",
        ),
        (
            "[categories]\npedantc = \"warn\"\n",
            "[categories]: unknown category 'pedantc'. Did you mean 'pedantic'?",
        ),
        (
            "disable = [\"type-mismach\"]\n",
            "disable: unknown rule or category 'type-mismach'. Did you mean 'type-mismatch'?",
        ),
    ] {
        t.write(
            "sqlsift.toml",
            &format!("schema = [\"schema.sql\"]\n{config}"),
        );
        let run = t.run(&["check", "q.sql"]);
        run.assert_code(2);
        // miette wraps long messages: compare without the layout
        let squash = |s: &str| {
            s.chars()
                .filter(|c| !c.is_whitespace() && *c != '│')
                .collect::<String>()
        };
        assert!(
            squash(&run.stderr).contains(&squash(expected)),
            "expected {expected:?} in:\n{}",
            run.stderr
        );
    }
}

#[test]
fn rule_names_work_like_codes() {
    let t = with_users_schema("disable-name");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--disable",
        "column-not-found",
        "q.sql",
    ])
    .assert_code(0);
}

#[test]
fn warn_flag_reports_without_failing() {
    let t = with_users_schema("warn-flag");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "-W", "E0002", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("warning[E0002]")
        .assert_stderr_contains("0 error(s), 1 warning(s)");
}

#[test]
fn category_flag_sets_all_rules_of_the_category() {
    let t = with_users_schema("warn-category");
    t.write("q.sql", "SELECT nme FROM users;\nSELECT id FROM nope;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-W", "correctness", "q.sql"]);
    run.assert_code(0)
        .assert_stderr_contains("0 error(s), 2 warning(s)");
    // A rule's own level wins over its category's
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "correctness",
        "-D",
        "table-not-found",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_contains("1 error(s), 1 warning(s)");
}

#[test]
fn config_rules_and_categories_tables_set_levels() {
    let t = with_users_schema("cfg-rules");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\n\n[categories]\ncorrectness = \"warn\"\n\n[rules]\nE0001 = \"error\"\nambiguous-column = \"off\"\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\nSELECT id FROM nope;\n");
    let run = t.run(&["check", "--format", "json", "q.sql"]);
    run.assert_code(1);
    let json: Value = serde_json::from_str(&run.stdout).expect("json");
    let severities: Vec<(String, String)> = json["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|d| {
            (
                d["kind"].as_str().unwrap().to_string(),
                d["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        severities,
        [
            ("ColumnNotFound".to_string(), "warning".to_string()),
            ("TableNotFound".to_string(), "error".to_string()),
        ]
    );
}

#[test]
fn invalid_rule_level_in_config_is_an_error() {
    let t = with_users_schema("cfg-bad-level");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\n\n[rules]\nE0002 = \"loud\"\n",
    );
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("invalid rule level 'loud'");
}

#[test]
fn rules_subcommand_lists_the_registry() {
    let t = TempDir::new("rules-list");
    let run = t.run(&["rules"]);
    run.assert_code(0);
    for text in [
        "E0001",
        "table-not-found",
        "correctness",
        "E1000",
        "parse-error",
    ] {
        run.assert_stdout_contains(text);
    }
}

#[test]
fn disable_parse_error_code() {
    let t = with_users_schema("disable-e1000");
    t.write("q.sql", "SELEC id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--disable", "E1000", "q.sql"])
        .assert_code(0);
}

#[test]
fn dialect_mysql_accepts_backticks() {
    let t = with_users_schema("dialect-mysql");
    t.write("q.sql", "SELECT `id`, `name` FROM `users`;\n");
    t.run(&["check", "-s", "schema.sql", "--dialect", "mysql", "q.sql"])
        .assert_code(0);
}

#[test]
fn dialect_postgresql_rejects_backticks() {
    let t = with_users_schema("dialect-pg-bt");
    t.write("q.sql", "SELECT `id` FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--dialect",
        "postgresql",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_contains("error[E1000]");
}

#[test]
fn dialect_mysql_still_finds_missing_columns() {
    let t = with_users_schema("dialect-mysql-err");
    t.write("q.sql", "SELECT `nme` FROM `users`;\n");
    t.run(&["check", "-s", "schema.sql", "-d", "mysql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0002]");
}

#[test]
fn dialect_sqlite() {
    let t = TempDir::new("dialect-sqlite");
    t.write(
        "schema.sql",
        "CREATE TABLE notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT);\n",
    );
    t.write("ok.sql", "SELECT id, body FROM notes;\n");
    t.write("bad.sql", "SELECT title FROM notes;\n");
    t.run(&["check", "-s", "schema.sql", "-d", "sqlite", "ok.sql"])
        .assert_code(0);
    t.run(&["check", "-s", "schema.sql", "-d", "sqlite", "bad.sql"])
        .assert_code(1);
}

#[test]
fn dialect_aliases_and_case_insensitivity() {
    let t = with_users_schema("dialect-alias");
    t.write("q.sql", "SELECT id FROM users;\n");
    for d in ["PostgreSQL", "postgres", "pg", "MySQL", "sqlite3"] {
        t.run(&["check", "-s", "schema.sql", "-d", d, "q.sql"])
            .assert_code(0);
    }
}

#[test]
fn dialect_invalid_exits_two_with_supported_list() {
    let t = with_users_schema("dialect-invalid");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--dialect", "oracle", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Unknown dialect: 'oracle'")
        .assert_stderr_contains("postgresql, mysql, sqlite");
}

// ---------------------------------------------------------------------------
// Human output
// ---------------------------------------------------------------------------

#[test]
fn human_output_has_location_code_source_and_help() {
    let t = with_users_schema("human");
    t.write("q.sql", "SELECT\n  nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("error[E0002]: Column 'nme' not found")
        .assert_stderr_contains("--> q.sql:2:3")
        .assert_stderr_contains("|   nme FROM users;")
        .assert_stderr_contains("^^^")
        .assert_stderr_contains("= help: Did you mean 'name'?");
}

#[test]
fn human_output_goes_to_stderr_only() {
    let t = with_users_schema("human-stderr");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1);
    assert!(run.stdout.is_empty(), "stdout: {}", run.stdout);
}

#[test]
fn human_output_table_not_found_location() {
    let t = with_users_schema("human-table");
    t.write("q.sql", "SELECT 1;\n\nSELECT id FROM orderz;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0001]: Table 'orderz' not found")
        .assert_stderr_contains("--> q.sql:3:16");
}

#[test]
fn human_output_paths_preserve_relative_subdirectory() {
    let t = with_users_schema("human-subdir");
    t.write("queries/reports/q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "queries/reports/q.sql"])
        .assert_code(1)
        .assert_stderr_contains("--> queries/reports/q.sql:1:8");
}

#[test]
fn human_output_type_mismatch_e0003() {
    let t = with_users_schema("human-e0003");
    t.write("q.sql", "SELECT id FROM users WHERE id = 'abc';\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0003]");
}

#[test]
fn human_output_insert_column_count_e0005() {
    let t = with_users_schema("human-e0005");
    t.write("q.sql", "INSERT INTO users (id, name) VALUES (1);\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0005]");
}

#[test]
fn human_output_ambiguous_column_e0006() {
    let t = TempDir::new("human-e0006");
    t.write("schema.sql", &format!("{USERS_SCHEMA}{ORDERS_SCHEMA}"));
    t.write("q.sql", "SELECT id FROM users, orders;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0006]");
}

#[test]
fn human_output_join_type_mismatch_e0007() {
    let t = TempDir::new("human-e0007");
    t.write(
        "schema.sql",
        "CREATE TABLE a (id INTEGER, code TEXT);\nCREATE TABLE b (id INTEGER, a_ref INTEGER);\n",
    );
    t.write("q.sql", "SELECT a.id FROM a JOIN b ON a.code = b.a_ref;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0007]");
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

#[test]
fn json_output_structure() {
    let t = with_users_schema("json");
    t.write("q.sql", "SELECT\n  nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "--format", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    assert_eq!(v["files"][0]["file"], "q.sql");
    let diags = v["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert_eq!(diags.len(), 1);
    let d = &diags[0];
    assert_eq!(d["kind"], "ColumnNotFound");
    assert_eq!(d["severity"], "error");
    assert_eq!(d["message"], "Column 'nme' not found in table 'users'");
    assert_eq!(d["help"], "Did you mean 'name'?");
    assert_eq!(d["span"]["line"], 2);
    assert_eq!(d["span"]["column"], 3);
    assert_eq!(d["span"]["length"], 3);
    assert!(d["labels"].is_array());
}

#[test]
fn json_output_help_is_null_when_absent() {
    let t = with_users_schema("json-nohelp");
    t.write("q.sql", "SELEC 1;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let d = &v["files"][0]["diagnostics"][0];
    assert_eq!(d["kind"], "ParseError");
    assert!(d.get("help").is_some(), "help key should be present");
}

#[test]
fn json_output_keeps_summary_on_stderr() {
    let t = with_users_schema("json-summary");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Found 1 error(s)");
    assert!(!run.stdout.contains("error(s)"), "stdout: {}", run.stdout);
}

#[test]
fn json_output_multiple_diagnostics_in_source_order() {
    let t = with_users_schema("json-order");
    t.write(
        "q.sql",
        "SELECT a FROM users;\nSELECT b FROM users;\nSELECT c FROM users;\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let lines: Vec<u64> = v["files"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["span"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, vec![1, 2, 3]);
}

#[test]
fn json_output_respects_disable() {
    let t = with_users_schema("json-disable");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'x';\n",
    );
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-f",
        "json",
        "--disable",
        "E0002",
        "q.sql",
    ]);
    run.assert_code(1);
    assert_eq!(diag_codes(&run.json()), vec!["TypeMismatch"]);
}

#[test]
fn json_output_only_for_files_with_diagnostics() {
    let t = with_users_schema("json-onefile");
    t.write("good.sql", "SELECT id FROM users;\n");
    t.write("bad.sql", "SELECT nme FROM users;\n");
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-f",
        "json",
        "good.sql",
        "bad.sql",
    ]);
    run.assert_code(1);
    let v = run.json();
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["files"][0]["file"], "bad.sql");
}

// ---------------------------------------------------------------------------
// SARIF output
// ---------------------------------------------------------------------------

#[test]
fn sarif_output_top_level_structure() {
    let t = with_users_schema("sarif");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "--format", "sarif", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    assert_eq!(v["version"], "2.1.0");
    assert!(v["$schema"]
        .as_str()
        .expect("$schema string")
        .contains("sarif-schema-2.1.0"));
    let runs = v["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], "sqlsift");
    assert_eq!(driver["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn sarif_results_have_rule_level_message_and_region() {
    let t = with_users_schema("sarif-results");
    t.write("queries/q.sql", "SELECT 1;\nSELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "sarif", "queries/q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let results = v["runs"][0]["results"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r["ruleId"], "E0002");
    assert_eq!(r["level"], "error");
    assert_eq!(
        r["message"]["text"],
        "Column 'nme' not found in table 'users'"
    );
    let loc = &r["locations"][0]["physicalLocation"];
    assert_eq!(loc["artifactLocation"]["uri"], "queries/q.sql");
    assert_eq!(loc["region"]["startLine"], 2);
    assert_eq!(loc["region"]["startColumn"], 8);
    assert_eq!(loc["region"]["endColumn"], 11);
}

#[test]
fn sarif_multiple_rule_ids() {
    let t = with_users_schema("sarif-multi");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'x';\nINSERT INTO users (id, name) VALUES (1);\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "-f", "sarif", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let ids: Vec<&str> = v["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ruleId"].as_str().unwrap())
        .collect();
    for code in ["E0002", "E0003", "E0005"] {
        assert!(ids.contains(&code), "missing {code} in {ids:?}");
    }
    for r in v["runs"][0]["results"].as_array().unwrap() {
        assert_eq!(r["level"], "error");
        assert!(r["message"]["text"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

// ---------------------------------------------------------------------------
// Inline suppression through the CLI
// ---------------------------------------------------------------------------

#[test]
fn inline_suppression_same_line() {
    let t = with_users_schema("suppress-same");
    t.write("q.sql", "SELECT nme FROM users; -- sqlsift:disable E0002\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn inline_suppression_next_line() {
    let t = with_users_schema("suppress-next");
    t.write(
        "q.sql",
        "-- sqlsift:disable E0002\nSELECT nme FROM users;\n",
    );
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn inline_suppression_only_affects_following_line() {
    let t = with_users_schema("suppress-scope");
    t.write(
        "q.sql",
        "-- sqlsift:disable E0002\nSELECT nme FROM users;\nSELECT other_bad FROM users;\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("other_bad")
        .assert_stderr_lacks("'nme'");
    assert_eq!(run.count_code("E0002"), 1);
}

#[test]
fn inline_suppression_wrong_code_does_not_suppress() {
    let t = with_users_schema("suppress-wrong");
    t.write("q.sql", "SELECT nme FROM users; -- sqlsift:disable E0001\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E0002]");
}

#[test]
fn inline_suppression_multiple_codes() {
    let t = with_users_schema("suppress-multi");
    t.write(
        "q.sql",
        "SELECT bad_col FROM missing_table; -- sqlsift:disable E0001, E0002\n",
    );
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn inline_suppression_all_rules() {
    let t = with_users_schema("suppress-all");
    t.write("q.sql", "-- sqlsift:disable\nSELECT nme FROM nope;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(0);
}

#[test]
fn inline_suppression_applies_to_json_output() {
    let t = with_users_schema("suppress-json");
    t.write(
        "q.sql",
        "SELECT nme FROM users; -- sqlsift:disable E0002\nSELECT zzz FROM users;\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let diags = v["files"][0]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["span"]["line"], 2);
}

// ---------------------------------------------------------------------------
// Regression tests for previously-found CLI bugs
// ---------------------------------------------------------------------------

/// Find the source line and caret line of the first human diagnostic and
/// return (column of `token` in the source line, column of first `^`).
fn caret_columns(stderr: &str, token: &str) -> (usize, usize) {
    let lines: Vec<&str> = stderr.lines().collect();
    let caret_idx = lines
        .iter()
        .position(|l| l.contains('^'))
        .unwrap_or_else(|| panic!("no caret line in:\n{stderr}"));
    let src = lines[caret_idx - 1];
    let src_col = src
        .find(token)
        .unwrap_or_else(|| panic!("token {token:?} not in {src:?}"));
    let caret_col = lines[caret_idx].find('^').unwrap();
    (src_col, caret_col)
}

#[test]
fn json_zero_diagnostics_is_valid_json() {
    let t = with_users_schema("json-empty");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(0);
    let v = run.json();
    assert_eq!(v["files"], serde_json::json!([]));
}

#[test]
fn sarif_zero_results_is_valid_sarif() {
    let t = with_users_schema("sarif-empty");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "sarif", "q.sql"]);
    run.assert_code(0);
    let v = run.json();
    assert_eq!(v["version"], "2.1.0");
    assert_eq!(v["runs"][0]["results"], serde_json::json!([]));
    assert_eq!(v["runs"][0]["tool"]["driver"]["name"], "sqlsift");
}

#[test]
fn json_multiple_files_form_one_document() {
    let t = with_users_schema("json-multi");
    t.write("a.sql", "SELECT a1 FROM users;\n");
    t.write("b.sql", "SELECT b1 FROM users;\nSELECT b2 FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "a.sql", "b.sql"]);
    run.assert_code(1);
    let v = run.json();
    let files = v["files"].as_array().expect("files array");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["file"], "a.sql");
    assert_eq!(files[0]["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(files[1]["file"], "b.sql");
    assert_eq!(files[1]["diagnostics"].as_array().unwrap().len(), 2);
}

#[test]
fn sarif_multiple_files_form_one_run() {
    let t = with_users_schema("sarif-multi-file");
    t.write("a.sql", "SELECT a1 FROM users;\n");
    t.write("b.sql", "SELECT b1 FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "sarif", "a.sql", "b.sql"]);
    run.assert_code(1);
    let v = run.json();
    let runs = v["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let uris: Vec<&str> = runs[0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(uris, vec!["a.sql", "b.sql"]);
}

#[test]
fn sarif_rules_describe_every_result_rule() {
    let t = with_users_schema("sarif-rules");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\nSELECT id FROM nope;\nSELEC;\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "-f", "sarif", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let rules = v["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .expect("driver.rules array");
    for rule in rules {
        assert!(rule["id"].as_str().is_some_and(|s| s.starts_with('E')));
        assert!(rule["name"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(rule["shortDescription"]["text"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(rule["helpUri"]
            .as_str()
            .is_some_and(|s| s.starts_with("https://")));
    }
    let results = v["runs"][0]["results"].as_array().unwrap();
    assert!(!results.is_empty());
    for r in results {
        let idx = r["ruleIndex"].as_u64().expect("ruleIndex") as usize;
        assert_eq!(rules[idx]["id"], r["ruleId"], "ruleIndex mismatch: {r}");
    }
    let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
    for code in ["E0001", "E0002", "E1000"] {
        assert!(ids.contains(&code), "{code} missing from {ids:?}");
    }
}

#[test]
fn json_diagnostic_has_code_line_and_column() {
    let t = with_users_schema("json-code");
    t.write("q.sql", "SELECT 1;\n  SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let d = &v["files"][0]["diagnostics"][0];
    assert_eq!(d["code"], "E0002");
    assert_eq!(d["line"], 2);
    assert_eq!(d["column"], 10);
    // Backwards-compatible fields are kept.
    assert_eq!(d["kind"], "ColumnNotFound");
    assert_eq!(d["span"]["line"], 2);
    assert_eq!(d["severity"], "error");
}

#[test]
fn verbose_logs_go_to_stderr_and_keep_json_valid() {
    let t = with_users_schema("v-json");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["-vv", "check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Loaded sqlsift configuration");
    assert_eq!(diag_codes(&run.json()), vec!["ColumnNotFound"]);
}

#[test]
fn no_ansi_escapes_when_not_a_terminal() {
    let t = with_users_schema("no-ansi");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let output = Command::new(env!("CARGO_BIN_EXE_sqlsift"))
        .current_dir(t.path())
        .args(["-v", "check", "-s", "schema.sql", "q.sql"])
        .env_remove("NO_COLOR")
        .output()
        .expect("run sqlsift");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[E0002]"), "{stderr}");
    assert!(
        !stderr.contains('\u{1b}'),
        "unexpected ANSI escape:\n{stderr}"
    );
}

#[test]
fn no_ansi_escapes_with_no_color() {
    let t = with_users_schema("no-color");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1);
    assert!(!run.raw_stderr.contains('\u{1b}'), "{}", run.raw_stderr);
}

#[test]
fn caret_is_aligned_under_token() {
    let t = with_users_schema("caret");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1);
    let (src, caret) = caret_columns(&run.stderr, "nme");
    assert_eq!(src, caret, "stderr:\n{}", run.stderr);
}

#[test]
fn caret_is_aligned_for_multi_digit_line_numbers() {
    let t = with_users_schema("caret-wide");
    let mut sql = "SELECT 1;\n".repeat(1200);
    sql.push_str("SELECT id, zzz FROM users;\n");
    t.write("q.sql", &sql);
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1).assert_stderr_contains("q.sql:1201:12");
    let (src, caret) = caret_columns(&run.stderr, "zzz");
    assert_eq!(src, caret, "stderr:\n{}", run.stderr);
}

#[test]
fn config_dialect_is_used() {
    let t = with_users_schema("cfg-dialect");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ndialect = \"mysql\"\n",
    );
    t.write("q.sql", "SELECT `id` FROM `users`;\n");
    t.run(&["check", "q.sql"]).assert_code(0);
}

#[test]
fn cli_dialect_overrides_config_dialect() {
    let t = with_users_schema("cfg-dialect-override");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ndialect = \"mysql\"\n",
    );
    t.write("q.sql", "SELECT `id` FROM users;\n");
    t.run(&["check", "-d", "postgresql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("error[E1000]");
}

#[test]
fn config_invalid_dialect_exits_two() {
    let t = with_users_schema("cfg-dialect-bad");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ndialect = \"oracle\"\n",
    );
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Unknown dialect: 'oracle'")
        .assert_stderr_contains("postgresql, mysql, sqlite");
}

#[test]
fn config_relative_paths_resolve_against_config_dir_when_discovered_in_parent() {
    let t = with_users_schema("cfg-rel-parent");
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\n");
    t.write("sub/deeper/q.sql", "SELECT nme FROM users;\n");
    let cwd = t.path().join("sub/deeper");
    t.run_in(&cwd, &["check", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Column 'nme' not found")
        .assert_stderr_contains("--> q.sql:1:8");
}

#[test]
fn explicit_config_relative_paths_resolve_against_config_dir() {
    let t = TempDir::new("cfg-rel-explicit");
    t.write("proj/db/schema.sql", USERS_SCHEMA);
    t.write("proj/queries/q.sql", "SELECT nme FROM users;\n");
    t.write(
        "proj/sqlsift.toml",
        "schema_dir = \"db\"\nfiles = [\"queries/*.sql\"]\n",
    );
    t.run(&["check", "--config", "proj/sqlsift.toml"])
        .assert_code(1)
        .assert_stderr_contains("--> proj/queries/q.sql:1:8");
}

#[test]
fn config_schema_entries_support_globs() {
    let t = TempDir::new("cfg-schema-glob");
    t.write("db/users.sql", USERS_SCHEMA);
    t.write("db/orders.sql", ORDERS_SCHEMA);
    t.write("sqlsift.toml", "schema = [\"db/*.sql\"]\n");
    t.write("q.sql", "SELECT u.id, o.total FROM users u, orders o;\n");
    t.run(&["check", "q.sql"]).assert_code(0);
}

#[test]
fn config_schema_glob_matching_nothing_exits_two() {
    let t = TempDir::new("cfg-schema-glob-none");
    t.write("sqlsift.toml", "schema = [\"db/*.sql\"]\n");
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("db/*.sql");
}

#[test]
fn query_patterns_with_question_mark_and_brackets_are_globbed() {
    let t = with_users_schema("glob-meta");
    t.write("q1.sql", "SELECT id FROM users;\n");
    t.write("q2.sql", "SELECT name FROM users;\n");
    t.write("qa.sql", "SELECT nope FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q?.sql"])
        .assert_code(1)
        .assert_stderr_contains("in 3 file(s)");
    t.run(&["check", "-s", "schema.sql", "q[12].sql"])
        .assert_code(0)
        .assert_stderr_contains("All 2 file(s) passed validation");
}

#[test]
fn missing_schema_file_error_names_the_path() {
    let t = TempDir::new("missing-schema-name");
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "-s", "nos.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("nos.sql");
}

#[test]
fn missing_query_file_error_names_the_path() {
    let t = with_users_schema("missing-query-name");
    t.run(&["check", "-s", "schema.sql", "ghost_query.sql"])
        .assert_code(2)
        .assert_stderr_contains("ghost_query.sql");
}

#[test]
fn missing_config_file_error_names_the_path() {
    let t = with_users_schema("missing-config-name");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-c", "ghost.toml", "-s", "schema.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("ghost.toml");
}

#[test]
fn missing_schema_dir_has_clear_error() {
    let t = TempDir::new("schema-dir-missing");
    t.write("q.sql", "SELECT 1;\n");
    t.run(&["check", "--schema-dir", "nodir", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Schema directory not found: nodir");
}

#[test]
fn max_errors_boundary_reports_unchecked_files_and_accurate_count() {
    let t = with_users_schema("max-boundary");
    t.write("one.sql", "SELECT nme FROM users;\n");
    t.write("ok.sql", "SELECT id FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--max-errors",
        "1",
        "one.sql",
        "ok.sql",
    ])
    .assert_code(1)
    .assert_stderr_contains("Reached maximum error limit (1). Stopped early.")
    .assert_stderr_contains("1 file(s) not checked")
    .assert_stderr_contains("Found 1 error(s), 0 warning(s) in 1 file(s)");
}

#[test]
fn config_invalid_format_exits_two() {
    let t = with_users_schema("cfg-format-bad");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nformat = \"xml\"\n",
    );
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Invalid format 'xml'")
        .assert_stderr_contains("human, json, sarif");
}

// ---------------------------------------------------------------------------
// JSON span offsets (#85)
// ---------------------------------------------------------------------------

#[test]
fn json_span_offset_is_the_byte_offset_of_the_location() {
    let t = with_users_schema("json-offset");
    let sql = "SELECT id FROM users;\n-- é\nSELECT 'ü', nme FROM users;\n";
    t.write("q.sql", sql);
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    run.assert_code(1);
    let v = run.json();
    let span = &v["files"][0]["diagnostics"][0]["span"];
    assert_eq!(span["line"], 3);
    assert_eq!(span["column"], 13);
    let offset = span["offset"].as_u64().expect("offset") as usize;
    assert_eq!(offset, sql.find("nme").unwrap());
    assert_eq!(&sql[offset..offset + 3], "nme");
}

// ---------------------------------------------------------------------------
// Reading a query from stdin (#91)
// ---------------------------------------------------------------------------

#[test]
fn stdin_dash_reads_query_from_stdin() {
    let t = with_users_schema("stdin");
    t.run_stdin(
        &["check", "-s", "schema.sql", "-"],
        "SELECT\n  nme FROM users;\n",
    )
    .assert_code(1)
    .assert_stderr_contains("error[E0002]")
    .assert_stderr_contains("--> <stdin>:2:3")
    .assert_stderr_contains("in 1 file(s)");
}

#[test]
fn stdin_valid_query_passes() {
    let t = with_users_schema("stdin-ok");
    t.run_stdin(
        &["check", "-s", "schema.sql", "-"],
        "SELECT id FROM users;\n",
    )
    .assert_code(0)
    .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn stdin_filename_is_used_in_diagnostics() {
    let t = with_users_schema("stdin-name");
    let run = t.run_stdin(
        &[
            "check",
            "-s",
            "schema.sql",
            "-f",
            "json",
            "--stdin-filename",
            "sql/users.sql",
            "-",
        ],
        "SELECT nme FROM users;\n",
    );
    run.assert_code(1);
    assert_eq!(run.json()["files"][0]["file"], "sql/users.sql");
}

#[test]
fn stdin_honors_inline_suppression() {
    let t = with_users_schema("stdin-suppress");
    t.run_stdin(
        &["check", "-s", "schema.sql", "-"],
        "SELECT nme FROM users; -- sqlsift:disable E0002\n",
    )
    .assert_code(0);
}

#[test]
fn stdin_can_be_combined_with_files() {
    let t = with_users_schema("stdin-mixed");
    t.write("bad.sql", "SELECT x FROM users;\n");
    t.run_stdin(
        &["check", "-s", "schema.sql", "bad.sql", "-"],
        "SELECT y FROM users;\n",
    )
    .assert_code(1)
    .assert_stderr_contains("bad.sql:1:8")
    .assert_stderr_contains("<stdin>:1:8")
    .assert_stderr_contains("Found 2 error(s), 0 warning(s) in 2 file(s)");
}

#[test]
fn stdin_given_twice_exits_two() {
    let t = with_users_schema("stdin-twice");
    t.run_stdin(&["check", "-s", "schema.sql", "-", "-"], "SELECT 1;\n")
        .assert_code(2)
        .assert_stderr_contains("'-' (stdin) can only be given once");
}

#[test]
fn stdin_filename_without_dash_exits_two() {
    let t = with_users_schema("stdin-name-nodash");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--stdin-filename",
        "x.sql",
        "q.sql",
    ])
    .assert_code(2)
    .assert_stderr_contains("--stdin-filename requires '-'");
}

// ---------------------------------------------------------------------------
// GitHub Actions output (#92)
// ---------------------------------------------------------------------------

#[test]
fn github_format_prints_one_workflow_command_per_diagnostic() {
    let t = with_users_schema("github");
    t.write(
        "sql/q.sql",
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'x';\n",
    );
    let run = t.run(&["check", "-s", "schema.sql", "-f", "github", "sql/q.sql"]);
    run.assert_code(1)
        .assert_stderr_contains("Found 2 error(s)");
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "::error file=sql/q.sql,line=1,col=8,endLine=1,endColumn=11,title=E0002 column-not-found::Column 'nme' not found in table 'users'%0Ahelp: Did you mean 'name'?",
            "::error file=sql/q.sql,line=2,col=28,endLine=2,endColumn=30,title=E0003 type-mismatch::Type mismatch: cannot compare integer with text%0Ahelp: Types are not implicitly compatible. Consider using explicit CAST.",
        ],
        "stdout:\n{}",
        run.stdout
    );
    // The human-readable diagnostics are not printed as well
    run.assert_stderr_lacks("-->");
}

#[test]
fn github_format_reports_warnings_as_warning_commands() {
    let t = with_users_schema("github-warn");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-f",
        "github",
        "-W",
        "E0002",
        "q.sql",
    ]);
    run.assert_code(0);
    assert!(
        run.stdout.starts_with("::warning file=q.sql,line=1,col=8,"),
        "stdout:\n{}",
        run.stdout
    );
}

#[test]
fn github_format_escapes_properties_and_data() {
    let t = with_users_schema("github-escape");
    // (`:` isn't allowed in Windows file names, so the name comes from --stdin-filename)
    let run = t.run_stdin(
        &[
            "check",
            "-s",
            "schema.sql",
            "-f",
            "github",
            "--stdin-filename",
            "a,b:c%.sql",
            "-",
        ],
        "SELECT nme FROM users;\n",
    );
    run.assert_code(1);
    assert!(
        run.stdout
            .starts_with("::error file=a%2Cb%3Ac%25.sql,line=1,col=8,"),
        "stdout:\n{}",
        run.stdout
    );
    // The title has nothing to escape; the message follows the `::` separator
    assert!(run.stdout.contains("title=E0002 column-not-found::"));
}

#[test]
fn github_format_with_no_diagnostics_prints_nothing() {
    let t = with_users_schema("github-clean");
    t.write("q.sql", "SELECT id FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "github", "q.sql"]);
    run.assert_code(0);
    assert_eq!(run.stdout, "");
}

#[test]
fn github_format_from_config() {
    let t = with_users_schema("github-cfg");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nformat = \"github\"\n",
    );
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "q.sql"])
        .assert_code(1)
        .assert_stdout_contains("::error file=q.sql,line=1,col=8,");
}

// ---------------------------------------------------------------------------
// --max-warnings (#93)
// ---------------------------------------------------------------------------

/// Temp dir with a query that produces two warnings when E0002 is set to warn
fn two_warnings(prefix: &str) -> TempDir {
    let t = with_users_schema(prefix);
    t.write("q.sql", "SELECT a FROM users;\nSELECT b FROM users;\n");
    t
}

#[test]
fn warnings_do_not_fail_without_max_warnings() {
    let t = two_warnings("maxw-none");
    t.run(&["check", "-s", "schema.sql", "-W", "E0002", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("Found 0 error(s), 2 warning(s) in 1 file(s)");
}

#[test]
fn max_warnings_exceeded_exits_one_with_reason() {
    let t = two_warnings("maxw-exceeded");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--max-warnings",
        "1",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_contains("warning[E0002]")
    .assert_stderr_contains("Found 0 error(s), 2 warning(s) in 1 file(s)")
    .assert_stderr_contains("Too many warnings: 2 found, the maximum is 1 (--max-warnings)");
}

#[test]
fn max_warnings_equal_to_count_passes() {
    let t = two_warnings("maxw-equal");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--max-warnings",
        "2",
        "q.sql",
    ])
    .assert_code(0)
    .assert_stderr_lacks("Too many warnings");
}

#[test]
fn max_warnings_zero_fails_on_any_warning() {
    let t = two_warnings("maxw-zero");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--max-warnings",
        "0",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_contains("Too many warnings: 2 found, the maximum is 0");
}

#[test]
fn max_warnings_reason_is_printed_even_when_quiet() {
    let t = two_warnings("maxw-quiet");
    t.run(&[
        "check",
        "-q",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--max-warnings",
        "1",
        "q.sql",
    ])
    .assert_code(1)
    .assert_stderr_lacks("Found 0 error(s)")
    .assert_stderr_contains("Too many warnings");
}

#[test]
fn max_warnings_from_config() {
    let t = two_warnings("maxw-cfg");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nmax_warnings = 1\n\n[rules]\nE0002 = \"warn\"\n",
    );
    t.run(&["check", "q.sql"])
        .assert_code(1)
        .assert_stderr_lacks("unknown key")
        .assert_stderr_contains("Too many warnings: 2 found, the maximum is 1");
}

#[test]
fn max_warnings_cli_overrides_config() {
    let t = two_warnings("maxw-cfg-override");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nmax_warnings = 0\n\n[rules]\nE0002 = \"warn\"\n",
    );
    t.run(&["check", "--max-warnings", "5", "q.sql"])
        .assert_code(0);
}

#[test]
fn max_warnings_invalid_config_value_exits_two() {
    let t = two_warnings("maxw-cfg-bad");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nmax_warnings = -1\n",
    );
    t.run(&["check", "q.sql"]).assert_code(2);
}

// ---------------------------------------------------------------------------
// Ignoring files (`ignore`, `--ignore`) and `-- sqlsift:disable-file`
// ---------------------------------------------------------------------------

#[test]
fn config_ignore_skips_matching_files_from_config_files() {
    let t = with_users_schema("ignore-cfg");
    t.write("sql/a.sql", "SELECT id FROM users;\n");
    t.write("sql/archive/old.sql", "SELECT nme FROM users;\n");
    t.write("sql/archive/2020/older.sql", "SELECT nme FROM users;\n");
    t.write("sql/users.generated.sql", "SELECT nme FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"sql/**/*.sql\"]\n\
         ignore = [\"sql/archive/**\", \"**/*.generated.sql\"]\n",
    );
    t.run(&["check"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn config_ignore_applies_to_positional_arguments() {
    let t = with_users_schema("ignore-positional");
    t.write("ok.sql", "SELECT id FROM users;\n");
    t.write("gen/bad.sql", "SELECT nme FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nignore = [\"gen\"]\n",
    );
    t.run(&["check", "ok.sql", "./gen/bad.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
    t.run(&["check", "gen/*.sql", "ok.sql"])
        .assert_code(0)
        .assert_stderr_lacks("nme");
}

#[test]
fn config_ignore_is_relative_to_config_dir() {
    let t = TempDir::new("ignore-rel");
    t.write("proj/schema.sql", USERS_SCHEMA);
    t.write("proj/gen/bad.sql", "SELECT nme FROM users;\n");
    t.write("proj/ok.sql", "SELECT id FROM users;\n");
    t.write(
        "proj/sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"**/*.sql\"]\nignore = [\"gen/**\", \"schema.sql\"]\n",
    );
    // Discovered from a subdirectory
    t.run_in(&t.path().join("proj/gen"), &["check"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
    // Given with --config from outside the project
    t.run(&["check", "--config", "proj/sqlsift.toml"])
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn cli_ignore_flag_is_repeatable_and_adds_to_config() {
    let t = with_users_schema("ignore-cli");
    t.write("a.sql", "SELECT nme FROM users;\n");
    t.write("b.sql", "SELECT nme FROM users;\n");
    t.write("c.sql", "SELECT nme FROM users;\n");
    t.write("ok.sql", "SELECT id FROM users;\n");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nignore = [\"a.sql\"]\n",
    );
    let files = ["a.sql", "b.sql", "c.sql", "ok.sql"];
    let mut args = vec!["check", "--ignore", "b.sql", "--ignore", "c.sql"];
    args.extend(files);
    t.run(&args)
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
    // Without the flags only the config's pattern applies
    let mut args = vec!["check"];
    args.extend(files);
    t.run(&args)
        .assert_code(1)
        .assert_stderr_contains("Found 2 error(s), 0 warning(s) in 3 file(s)");
    // Glob syntax in the flag
    let mut args = vec!["check", "--ignore", "[bc].sql"];
    args.extend(files);
    t.run(&args)
        .assert_code(0)
        .assert_stderr_contains("All 1 file(s) passed validation");
}

#[test]
fn all_files_ignored_is_not_an_error() {
    let t = with_users_schema("ignore-all");
    t.write("gen/bad.sql", "SELECT nme FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--ignore",
        "gen/**",
        "gen/bad.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains("No files to check (1 ignored)");
}

#[test]
fn invalid_ignore_pattern_exits_two() {
    let t = with_users_schema("ignore-invalid");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--ignore", "[x.sql", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("invalid ignore pattern '[x.sql'");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nignore = [\"[x.sql\"]\n",
    );
    t.run(&["check", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("invalid ignore pattern '[x.sql'");
}

#[test]
fn ignore_is_a_known_config_key() {
    let t = with_users_schema("ignore-known");
    t.write("q.sql", "SELECT id FROM users;\n");
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\nignore = []\n");
    t.run(&["check", "q.sql"])
        .assert_code(0)
        .assert_stderr_lacks("unknown key");
}

#[test]
fn disable_file_directive_suppresses_rules_for_the_whole_file() {
    let t = with_users_schema("disable-file");
    t.write(
        "q.sql",
        "SELECT nme FROM users;\n-- sqlsift:disable-file column-not-found\nSELECT other FROM users;\nSELECT id FROM nope;\n",
    );
    t.write("other.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "q.sql", "other.sql"])
        .assert_code(1)
        .assert_stderr_contains("Found 2 error(s)")
        .assert_stderr_contains("q.sql:4:16")
        .assert_stderr_contains("other.sql:1:8")
        .assert_stderr_lacks("q.sql:1:8");
}

#[test]
fn disable_file_directive_suppresses_parse_errors_and_everything() {
    let t = with_users_schema("disable-file-all");
    t.write(
        "q.sql",
        "-- sqlsift:disable-file\nSELECT nme FROM users;\nSELEC broken;\n",
    );
    t.write(
        "p.sql",
        "-- sqlsift:disable-file E1000\nSELECT id FROM users;\nSELEC broken;\n",
    );
    t.run(&["check", "-s", "schema.sql", "q.sql", "p.sql"])
        .assert_code(0)
        .assert_stderr_contains("All 2 file(s) passed validation");
}

// ---------------------------------------------------------------------------
// Baseline (#97)
// ---------------------------------------------------------------------------

/// Temp dir with a query file that has two errors (E0002 and E0001)
fn with_two_errors(prefix: &str) -> TempDir {
    let t = with_users_schema(prefix);
    t.write("q.sql", "SELECT nme FROM users;\nSELECT id FROM userz;\n");
    t
}

fn read_baseline(t: &TempDir, rel: &str) -> Value {
    let text = fs::read_to_string(t.path().join(rel)).expect("baseline file written");
    serde_json::from_str(&text).expect("baseline is JSON")
}

#[test]
fn write_baseline_records_every_diagnostic_and_exits_zero() {
    let t = with_two_errors("bl-write");
    t.write("w.sql", "SELECT nme FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--write-baseline",
        "q.sql",
        "w.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains("Wrote 3 baseline entries for 2 file(s) to sqlsift-baseline.json")
    .assert_stderr_contains("--baseline sqlsift-baseline.json");
    let baseline = read_baseline(&t, "sqlsift-baseline.json");
    assert_eq!(baseline["version"], 1);
    let entries = baseline["entries"].as_array().unwrap();
    // Warnings are recorded too
    let summary: Vec<(String, String, u64)> = entries
        .iter()
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_string(),
                e["code"].as_str().unwrap().to_string(),
                e["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("q.sql".to_string(), "E0002".to_string(), 1),
            ("q.sql".to_string(), "E0001".to_string(), 2),
            ("w.sql".to_string(), "E0002".to_string(), 1),
        ]
    );
    assert!(entries[0]["message"].as_str().unwrap().contains("nme"));
    assert_eq!(entries[0]["statement_hash"].as_str().unwrap().len(), 16);
}

#[test]
fn baselined_diagnostics_are_not_reported() {
    let t = with_two_errors("bl-check");
    t.run(&["check", "-s", "schema.sql", "--write-baseline", "q.sql"])
        .assert_code(0);
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--baseline",
        "sqlsift-baseline.json",
        "q.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains("All 1 file(s) passed validation")
    .assert_stderr_contains("2 known diagnostic(s) hidden by the baseline")
    .assert_stderr_lacks("error[");
}

#[test]
fn baseline_survives_lines_added_above_and_reports_new_problems() {
    let t = with_two_errors("bl-shift");
    t.run(&["check", "-s", "schema.sql", "--write-baseline", "q.sql"])
        .assert_code(0);
    t.write(
        "q.sql",
        "-- a new comment\nSELECT id FROM users;\n\nSELECT nme\n  FROM users;\nSELECT id FROM userz;\nSELECT bad FROM users;\n",
    );
    let run = t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--baseline",
        "sqlsift-baseline.json",
        "-f",
        "json",
        "q.sql",
    ]);
    run.assert_code(1)
        .assert_stderr_contains("Found 1 error(s), 0 warning(s) in 1 file(s)")
        .assert_stderr_contains("2 known diagnostic(s) hidden by the baseline");
    let json = run.json();
    assert_eq!(diag_codes(&json), ["ColumnNotFound"]);
    assert!(run.stdout.contains("bad"), "{}", run.stdout);
}

#[test]
fn fixed_baseline_entries_are_a_note_not_a_failure() {
    let t = with_two_errors("bl-stale");
    t.run(&["check", "-s", "schema.sql", "--write-baseline", "q.sql"])
        .assert_code(0);
    t.write("q.sql", "SELECT name FROM users;\nSELECT id FROM userz;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--baseline",
        "sqlsift-baseline.json",
        "q.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains(
        "Note: 1 baseline entry no longer occurs; re-run with --write-baseline to remove it",
    );
}

#[test]
fn baseline_entries_of_unchecked_files_are_not_stale() {
    let t = with_two_errors("bl-subset");
    t.write("p.sql", "SELECT x FROM users;\n");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--write-baseline",
        "q.sql",
        "p.sql",
    ])
    .assert_code(0);
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--baseline",
        "sqlsift-baseline.json",
        "p.sql",
    ])
    .assert_code(0)
    .assert_stderr_lacks("no longer occur");
}

#[test]
fn baseline_from_config_works_from_a_subdirectory() {
    let t = with_two_errors("bl-config");
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nfiles = [\"sql/*.sql\"]\nbaseline = \"ci/baseline.json\"\n",
    );
    t.write("sql/q.sql", "SELECT nme FROM users;\n");
    t.mkdir("ci");
    t.run(&["check", "--write-baseline"])
        .assert_code(0)
        .assert_stderr_contains("Wrote 1 baseline entry for 1 file(s)")
        .assert_stderr_lacks("--baseline");
    let baseline = read_baseline(&t, "ci/baseline.json");
    assert_eq!(baseline["entries"][0]["file"], "../sql/q.sql");
    // Same file names whatever the working directory
    t.run_in(&t.path().join("sql"), &["check", "q.sql"])
        .assert_code(0)
        .assert_stderr_contains("1 known diagnostic(s) hidden by the baseline");
    t.run(&["check"]).assert_code(0);
}

#[test]
fn missing_baseline_file_exits_two() {
    let t = with_two_errors("bl-missing");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--baseline",
        "nope.json",
        "q.sql",
    ])
    .assert_code(2)
    .assert_stderr_contains("Baseline file not found: nope.json")
    .assert_stderr_contains("--write-baseline");
}

#[test]
fn invalid_baseline_file_exits_two() {
    let t = with_two_errors("bl-invalid");
    t.write("b.json", "{\"version\": 7, \"entries\": []}");
    t.run(&["check", "-s", "schema.sql", "--baseline", "b.json", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("unsupported baseline version 7");
}

#[test]
fn baselined_warnings_do_not_count_toward_max_warnings() {
    let t = two_warnings("bl-maxw");
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--write-baseline",
        "q.sql",
    ])
    .assert_code(0);
    t.write(
        "q.sql",
        "SELECT a FROM users;\nSELECT b FROM users;\nSELECT c FROM users;\n",
    );
    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "-W",
        "E0002",
        "--max-warnings",
        "1",
        "--baseline",
        "sqlsift-baseline.json",
        "q.sql",
    ])
    .assert_code(0)
    .assert_stderr_contains("Found 0 error(s), 1 warning(s) in 1 file(s)");
}

#[test]
fn baseline_matches_stdin_by_stdin_filename() {
    let t = with_users_schema("bl-stdin");
    t.write("q.sql", "SELECT nme FROM users;\n");
    t.run(&["check", "-s", "schema.sql", "--write-baseline", "q.sql"])
        .assert_code(0);
    t.run_stdin(
        &[
            "check",
            "-s",
            "schema.sql",
            "--baseline",
            "sqlsift-baseline.json",
            "--stdin-filename",
            "q.sql",
            "-",
        ],
        "SELECT nme FROM users;\n",
    )
    .assert_code(0);
}

// ---------------------------------------------------------------------------
// dbt / Jinja templating
// ---------------------------------------------------------------------------

/// The dbt-like project in `tests/fixtures/dbt`
fn dbt_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dbt")
        .canonicalize()
        .expect("dbt fixture exists")
}

const DBT_MODELS: [&str; 2] = [
    "models/staging/stg_orders.sql",
    "models/marts/customer_orders.sql",
];

#[test]
fn dbt_project_in_current_directory_enables_jinja() {
    let t = TempDir::new("dbt-cwd");
    let dbt = dbt_fixture();
    let mut args = vec!["check", "--schema", "schema.sql"];
    args.extend(DBT_MODELS);
    t.run_in(&dbt, &args)
        .assert_code(0)
        .assert_stderr_contains("All 2 file(s) passed validation");

    // A typo in a real table's column is still reported at its original location
    let run = t.run_in(
        &dbt,
        &[
            "check",
            "--schema",
            "schema.sql",
            "--format",
            "json",
            "models/marts/customer_typo.sql",
        ],
    );
    run.assert_code(1);
    let json = run.json();
    let diagnostics = json["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .clone();
    assert_eq!(diagnostics.len(), 1, "{json:#}");
    assert_eq!(diagnostics[0]["code"], "E0002");
    assert_eq!(diagnostics[0]["span"]["line"], 3);
    assert_eq!(diagnostics[0]["span"]["column"], 16);
}

#[test]
fn templating_none_turns_dbt_detection_off() {
    let t = TempDir::new("dbt-none");
    let mut args = vec!["check", "--templating", "none", "--schema", "schema.sql"];
    args.extend(DBT_MODELS);
    let run = t.run_in(&dbt_fixture(), &args);
    run.assert_code(1);
    assert!(run.count_code("E1000") > 0, "{}", run.stderr);
}

#[test]
fn templating_flag_and_config_key() {
    let t = with_users_schema("jinja-config");
    t.write(
        "q.sql",
        "{{ config(materialized='view') }}\nSELECT u.id, e.kind FROM users u JOIN {{ ref('events') }} e ON e.user_id = u.id\n",
    );
    // Without templating (and no dbt_project.yml) the template doesn't parse
    let run = t.run(&["check", "-s", "schema.sql", "q.sql"]);
    run.assert_code(1);
    assert!(run.count_code("E1000") > 0, "{}", run.stderr);

    t.run(&[
        "check",
        "-s",
        "schema.sql",
        "--templating",
        "jinja",
        "q.sql",
    ])
    .assert_code(0);

    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\ntemplating = \"jinja\"\n",
    );
    t.run(&["check", "q.sql"])
        .assert_code(0)
        .assert_stderr_lacks("unknown key");

    t.run(&["check", "--templating", "mustache", "q.sql"])
        .assert_code(2)
        .assert_stderr_contains("Unknown templating: 'mustache'");
}

#[test]
fn dbt_project_next_to_config_file_enables_jinja() {
    let t = TempDir::new("dbt-config-dir");
    t.write("project/schema.sql", USERS_SCHEMA);
    t.write("project/sqlsift.toml", "schema = [\"schema.sql\"]\n");
    t.write("project/dbt_project.yml", "name: demo\n");
    t.write(
        "project/models/m.sql",
        "SELECT id FROM users WHERE id > {{ var('min_id') }}\n",
    );
    t.run(&[
        "check",
        "--config",
        "project/sqlsift.toml",
        "project/models/m.sql",
    ])
    .assert_code(0);
}

// ---------------------------------------------------------------------------
// SQL embedded in application code (tests/fixtures/embedded)
// ---------------------------------------------------------------------------

/// The repository root, where the `tests/fixtures` paths below are relative to
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Run `sqlsift check` on embedded-SQL fixtures, with their schema
fn check_embedded_fixture(extra: &[&str]) -> Run {
    let t = TempDir::new("embedded");
    let mut args = vec!["check", "-s", "tests/fixtures/embedded/schema.sql"];
    args.extend_from_slice(extra);
    t.run_in(&repository_root(), &args)
}

#[test]
fn sqlc_query_names_in_human_output() {
    check_embedded_fixture(&["tests/fixtures/embedded/queries.sql"])
        .assert_code(1)
        .assert_stderr_contains("queries.sql:8:12")
        .assert_stderr_contains("= note: in query 'ListPosts'")
        .assert_stderr_contains("queries.sql:18:22")
        .assert_stderr_contains("= note: in query 'GetAuthor'")
        .assert_stderr_lacks("'GetPost'");
}

#[test]
fn sqlc_query_names_in_json_output() {
    let run = check_embedded_fixture(&["-f", "json", "tests/fixtures/embedded/queries.sql"]);
    run.assert_code(1);
    let json = run.json();
    let names: Vec<&str> = json["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| d["query_name"].as_str().expect("query_name"))
        .collect();
    assert_eq!(names, ["ListPosts", "GetAuthor"]);
}

#[test]
fn json_output_has_no_query_name_outside_sqlc_queries() {
    let t = with_users_schema("no-query-name");
    t.write("q.sql", "SELECT nme FROM users;\n");
    let run = t.run(&["check", "-s", "schema.sql", "-f", "json", "q.sql"]);
    let json = run.json();
    assert!(json["files"][0]["diagnostics"][0]
        .get("query_name")
        .is_none());
}

#[test]
fn sqlc_query_names_in_sarif_output() {
    let run = check_embedded_fixture(&["-f", "sarif", "tests/fixtures/embedded/queries.sql"]);
    run.assert_code(1);
    let json = run.json();
    let result = &json["runs"][0]["results"][0];
    assert_eq!(
        result["message"]["text"],
        "Column 'titel' not found in table 'posts' (in query 'ListPosts')"
    );
    assert_eq!(
        result["locations"][0]["logicalLocations"][0]["name"],
        "ListPosts"
    );
    assert_eq!(
        result["locations"][0]["physicalLocation"]["region"]["startLine"],
        8
    );
}

#[test]
fn typescript_tagged_templates_are_checked() {
    check_embedded_fixture(&["tests/fixtures/embedded/queries.ts"])
        .assert_code(1)
        .assert_stderr_contains("queries.ts:16:28")
        .assert_stderr_contains(
            "16 |   return db.sql`SELECT id, titel FROM posts WHERE author_id = ${authorId} LIMIT ${limit}`;",
        )
        .assert_stderr_contains("queries.ts:31:10")
        .assert_stderr_contains("Found 2 error(s)")
        .assert_stderr_lacks("queries.ts:35");
}

#[test]
fn typescript_and_sql_files_are_matched_by_globs() {
    check_embedded_fixture(&["-f", "json", "tests/fixtures/embedded/*"])
        .assert_code(1)
        .assert_stdout_contains("queries.sql")
        .assert_stdout_contains("queries.ts");
}

#[test]
fn embedded_sql_tags_config_selects_the_tags() {
    let t = TempDir::new("embedded-tags");
    t.write("schema.sql", USERS_SCHEMA);
    t.write(
        "sqlsift.toml",
        "schema = [\"schema.sql\"]\nembedded_sql_tags = [\"$queryRaw\"]\n",
    );
    t.write(
        "src/db.ts",
        "const a = sql`SELECT nme FROM users`;\nconst b = prisma.$queryRaw<User[]>`SELECT nme FROM users WHERE id = ${id}`;\n",
    );
    let run = t.run(&["check", "-f", "json", "src/db.ts"]);
    run.assert_code(1).assert_stderr_lacks("unknown key");
    let json = run.json();
    let diagnostics = json["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["line"], 2);
    assert_eq!(diagnostics[0]["column"], 43);
    // The byte offset points into the TypeScript file
    assert_eq!(diagnostics[0]["span"]["offset"], 38 + 42);
}

#[test]
fn stdin_filename_extension_selects_typescript() {
    let t = with_users_schema("embedded-stdin");
    let source = "export const q = sql`SELECT nme FROM users`;\n";
    t.run_stdin(
        &["check", "-s", "schema.sql", "--stdin-filename", "q.ts", "-"],
        source,
    )
    .assert_code(1)
    .assert_stderr_contains("q.ts:1:29");
    // Without a TypeScript name, stdin is SQL
    t.run_stdin(&["check", "-s", "schema.sql", "-"], source)
        .assert_code(1)
        .assert_stderr_contains("E1000");
}

#[test]
fn schema_subcommand_shows_enum_schemas() {
    let t = TempDir::new("schema-cmd-enum-schema");
    t.write(
        "schema.sql",
        "CREATE TYPE public.st AS ENUM ('a', 'b');
CREATE SCHEMA billing;
CREATE TYPE billing.charge_state AS ENUM ('open', 'paid');
CREATE TABLE c (id int, s public.st);
",
    );
    t.run(&["schema", "-s", "schema.sql"])
        .assert_code(0)
        .assert_stdout_contains("public.st: 'a', 'b'")
        .assert_stdout_contains("billing.charge_state: 'open', 'paid'");
    let json = t
        .run(&["schema", "--format", "json", "-s", "schema.sql"])
        .json();
    assert_eq!(json["enums"][1]["name"], "charge_state");
    assert_eq!(json["enums"][1]["schema"], "billing");
}

#[test]
fn check_follows_postgres_script_statements() {
    let t = TempDir::new("check-postgres-scripts");
    t.write(
        "schema.sql",
        "CREATE TABLE users (id int, country text);
CREATE SCHEMA analytics;
CREATE TABLE analytics.daily_active (day date, dau int);
",
    );
    t.write(
        "q.sql",
        "COPY users (id, country) FROM stdin;
1\tJP
\\.
SELECT bogus FROM users;
SET search_path TO analytics, public;
SELECT dau FROM daily_active;
",
    );
    t.run(&["check", "-s", "schema.sql", "q.sql"])
        .assert_code(1)
        .assert_stderr_contains("Column 'bogus' not found")
        .assert_stderr_contains("Found 1 error(s), 0 warning(s) in 1 file(s)");
}
