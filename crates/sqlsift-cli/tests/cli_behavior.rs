use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn make_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX_EPOCH")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sqlsift-{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).expect("failed to write file");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("failed to resolve workspace root")
}

fn run_sqlsift(args: &[&str]) -> std::process::Output {
    Command::new("cargo")
        .current_dir(workspace_root())
        .args(["run", "-q", "-p", "sqlsift-cli", "--"])
        .args(args)
        .output()
        .expect("failed to execute sqlsift via cargo run")
}

#[test]
fn test_max_errors_stops_early() {
    let dir = make_temp_dir("max-errors");
    let schema = dir.join("schema.sql");
    let query = dir.join("query.sql");

    write_file(
        &schema,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    );
    write_file(
        &query,
        "SELECT missing_col_1 FROM users;\nSELECT missing_col_2 FROM users;\n",
    );

    let schema_s = schema.to_string_lossy().to_string();
    let query_s = query.to_string_lossy().to_string();
    let output = run_sqlsift(&[
        "check",
        "--max-errors",
        "1",
        "--schema",
        &schema_s,
        &query_s,
    ]);

    assert!(
        !output.status.success(),
        "expected non-zero exit when diagnostics exist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Reached maximum error limit (1). Stopped early."),
        "expected max-error limit message, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("missing_col_1"),
        "expected first diagnostic, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("missing_col_2"),
        "expected early-stop before second diagnostic, stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_quiet_suppresses_summary_output() {
    let dir = make_temp_dir("quiet");
    let schema = dir.join("schema.sql");
    let query = dir.join("query.sql");

    write_file(
        &schema,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    );
    write_file(&query, "SELECT missing_col FROM users;\n");

    let schema_s = schema.to_string_lossy().to_string();
    let query_s = query.to_string_lossy().to_string();
    let output = run_sqlsift(&["-q", "check", "--schema", &schema_s, &query_s]);

    assert!(
        !output.status.success(),
        "expected non-zero exit when diagnostics exist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing_col"),
        "expected diagnostic output, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Found "),
        "summary should be suppressed in quiet mode, stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("All "),
        "pass summary should be suppressed in quiet mode, stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_verbose_emits_info_log() {
    let dir = make_temp_dir("verbose");
    let schema = dir.join("schema.sql");
    let query = dir.join("query.sql");

    write_file(
        &schema,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    );
    write_file(&query, "SELECT id FROM users;\n");

    let schema_s = schema.to_string_lossy().to_string();
    let query_s = query.to_string_lossy().to_string();
    let output = run_sqlsift(&["-v", "check", "--schema", &schema_s, &query_s]);

    assert!(output.status.success(), "expected success for valid SQL");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Loaded sqlsift configuration"),
        "expected info-level log in verbose mode, stdout:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}
