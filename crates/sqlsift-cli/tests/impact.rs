//! End-to-end tests of `sqlsift impact`

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A project in a temporary directory, removed on drop
struct Project {
    path: PathBuf,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("sqlsift-impact-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        for (name, content) in files {
            let file = path.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        Self { path }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sqlsift"))
            .current_dir(&self.path)
            .arg("impact")
            .args(args)
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> (Value, Option<i32>) {
        let output = self.run(&[args, &["--format", "json"]].concat());
        let json = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "invalid JSON ({e}): {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (json, output.status.code())
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

const INIT: (&str, &str) = (
    "migrations/001_init.sql",
    "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);
     CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER);",
);
const RENAME: (&str, &str) = (
    "migrations/002_rename.sql",
    "ALTER TABLE users RENAME COLUMN email TO email_address;",
);
const QUERIES: [(&str, &str); 2] = [
    (
        "queries/users.sql",
        "SELECT id, email FROM users;\nSELECT email_address FROM users;\n",
    ),
    ("queries/orders.sql", "SELECT user_id FROM orders;\n"),
];

fn files(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    let mut files = vec![INIT, RENAME];
    files.extend(QUERIES);
    files.extend(extra);
    files
}

fn file_names(files: &Value) -> Vec<&str> {
    files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["file"].as_str().unwrap())
        .collect()
}

#[test]
fn migration_among_schema_files() {
    let project = Project::new(&files(&[]));
    let args = [
        "--schema-dir",
        "migrations",
        "--queries",
        "queries/*.sql",
        "migrations/002_rename.sql",
    ];
    let (json, code) = project.json(&args);
    assert_eq!(code, Some(1));
    assert_eq!(json["changes"][0]["name"], "public.users");
    assert_eq!(json["changes"][0]["kind"], "changed");
    assert_eq!(file_names(&json["introduced"]), ["queries/users.sql"]);
    let introduced = &json["introduced"][0]["diagnostics"][0];
    assert_eq!(introduced["code"], "E0002");
    assert_eq!(introduced["line"], 1);
    let resolved = &json["resolved"][0]["diagnostics"][0];
    assert_eq!(resolved["line"], 2);

    let human = project.run(&args);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(stderr.contains("changed table public.users"), "{stderr}");
    assert!(stderr.contains("renamed to 'email_address'"), "{stderr}");
    assert!(stderr.contains("Checked 1 of 2 query file(s)"), "{stderr}");
}

#[test]
fn later_migrations_are_left_out_of_before() {
    // 003 restores the column, so with 002 the queries are unaffected
    let project = Project::new(&files(&[(
        "migrations/003_back.sql",
        "ALTER TABLE users RENAME COLUMN email_address TO email;",
    )]));
    let (json, code) = project.json(&[
        "--schema-dir",
        "migrations",
        "--queries",
        "queries/*.sql",
        "migrations/003_back.sql",
    ]);
    assert_eq!(code, Some(1));
    assert_eq!(json["introduced"][0]["diagnostics"][0]["line"], 2);
}

#[test]
fn migration_outside_schema_files() {
    let project = Project::new(&[
        INIT,
        ("next/drop.sql", "DROP TABLE orders;"),
        QUERIES[0],
        QUERIES[1],
    ]);
    let (json, code) = project.json(&[
        "--schema",
        "migrations/001_init.sql",
        "--queries",
        "queries/*.sql",
        "next/drop.sql",
    ]);
    assert_eq!(code, Some(1));
    assert_eq!(json["changes"][0]["kind"], "removed");
    assert_eq!(file_names(&json["introduced"]), ["queries/orders.sql"]);
    assert_eq!(json["introduced"][0]["diagnostics"][0]["code"], "E0001");
}

#[test]
fn harmless_change_exits_zero() {
    let project = Project::new(&[
        INIT,
        (
            "migrations/002_add.sql",
            "ALTER TABLE users ADD COLUMN name TEXT;",
        ),
        QUERIES[0],
    ]);
    let (json, code) = project.json(&[
        "--schema-dir",
        "migrations",
        "--queries",
        "queries/*.sql",
        "migrations/002_add.sql",
    ]);
    assert_eq!(code, Some(0));
    assert_eq!(json["introduced"], Value::Array(Vec::new()));
}

#[test]
fn query_files_from_config() {
    let project = Project::new(&files(&[(
        "sqlsift.toml",
        "schema_dir = \"migrations\"\nfiles = [\"queries/*.sql\"]\n",
    )]));
    let (json, code) = project.json(&["migrations/002_rename.sql"]);
    assert_eq!(code, Some(1));
    assert_eq!(file_names(&json["introduced"]), ["queries/users.sql"]);
}

#[test]
fn missing_migration_is_an_error() {
    let project = Project::new(&files(&[]));
    let output = project.run(&[
        "--schema-dir",
        "migrations",
        "--queries",
        "queries/*.sql",
        "nope.sql",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("nope.sql"));
}
