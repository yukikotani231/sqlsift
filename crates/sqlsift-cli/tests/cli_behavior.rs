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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Loaded sqlsift configuration"),
        "expected info-level log on stderr in verbose mode, stderr:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Loaded sqlsift configuration"),
        "logs must not be written to stdout, stdout:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_schema_dir_skips_rollback_migrations() {
    let dir = make_temp_dir("rollbacks");
    let migrations = dir.join("migrations");
    fs::create_dir_all(migrations.join("nested")).expect("failed to create migrations dir");
    // golang-migrate / sqlx pair: the .down.sql sorts first and must be skipped
    write_file(
        &migrations.join("000001_users.down.sql"),
        "DROP TABLE users;",
    );
    write_file(
        &migrations.join("000001_users.up.sql"),
        "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL);",
    );
    // dbmate: up and down in one file
    write_file(
        &migrations.join("20240101000000_orders.sql"),
        "-- migrate:up\nCREATE TABLE orders (id INT, user_id INT);\n-- migrate:down\nDROP TABLE orders;\n",
    );
    // Flyway undo migration sorts after V2 and must be skipped
    write_file(
        &migrations.join("nested/V2__items.sql"),
        "CREATE TABLE items (id INT);",
    );
    write_file(
        &migrations.join("nested/U2__items.sql"),
        "DROP TABLE items;",
    );

    let query = dir.join("query.sql");
    write_file(
        &query,
        "SELECT u.id, u.name, o.id FROM users u JOIN orders o ON o.user_id = u.id;\nSELECT id FROM items;\n",
    );

    let migrations_s = migrations.to_string_lossy().to_string();
    let query_s = query.to_string_lossy().to_string();
    let output = run_sqlsift(&["check", "--schema-dir", &migrations_s, &query_s]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected rollback migrations to be ignored, stderr:\n{stderr}"
    );

    // An explicit --schema file is applied as given, even if it is a down file
    let up = migrations.join("000001_users.up.sql");
    let down = migrations.join("000001_users.down.sql");
    let users_query = dir.join("users_query.sql");
    write_file(&users_query, "SELECT id FROM users;");
    let output = run_sqlsift(&[
        "check",
        "--schema",
        &up.to_string_lossy(),
        "--schema",
        &down.to_string_lossy(),
        &users_query.to_string_lossy(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("E0001"),
        "expected an explicit down file to be applied, stderr:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
