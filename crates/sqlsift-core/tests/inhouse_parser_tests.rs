//! Experimental (#257): the checks that read the in-house parser's tree must report what
//! their sqlparser versions report. `cargo test -p sqlsift-core --features inhouse-parser`.
//!
//! The parser is switched for the whole process, so this file has one test.

#![cfg(feature = "inhouse-parser")]

use std::path::{Path, PathBuf};

use sqlsift_core::rules::{RuleConfig, RuleLevel};
use sqlsift_core::schema::Catalog;
use sqlsift_core::{parser_hook, Analyzer, Diagnostic, SqlDialect};

/// Statements for the restriction rules (E0024-E0026, E0028).
const RESTRICTIONS: &[&str] = &[
    "UPDATE users SET name = 'x'",
    "UPDATE public.\"Users\" u SET name = 'x' FROM t",
    "UPDATE users SET name = 'x' WHERE id = 1",
    "DELETE FROM users",
    "DELETE FROM s.users AS u USING t",
    "DELETE FROM users WHERE true",
    "INSERT INTO users VALUES (1, 'a', NULL)",
    "INSERT INTO s.\"Users\" SELECT * FROM archived",
    "INSERT INTO users (id) SELECT u.* FROM archived u",
    "INSERT INTO users DEFAULT VALUES",
    "INSERT INTO users VALUES (1) ON CONFLICT (id) DO NOTHING",
    "SELECT * FROM users",
    "SELECT u.*, a.id FROM users u, archived a",
    "SELECT s.users.* FROM s.users",
    "SELECT * FROM users UNION SELECT id FROM archived UNION (SELECT * FROM t)",
    "(SELECT * FROM users)",
    "SELECT id FROM (SELECT * FROM users) x WHERE id IN (SELECT * FROM t)",
    "WITH c AS (SELECT * FROM users) SELECT * FROM c",
    "WITH c AS (SELECT 1) INSERT INTO users SELECT * FROM c",
    "SELECT id FROM users LIMIT 10",
    "SELECT id FROM users LIMIT ALL",
    "SELECT id FROM users LIMIT ALL OFFSET 5",
    "SELECT id FROM users OFFSET 5 ROWS",
    "SELECT id FROM users LIMIT $1 OFFSET $2",
    "SELECT id FROM users LIMIT (5)",
    "SELECT id FROM users LIMIT -1",
    "SELECT id FROM users LIMIT 2 + 3",
    "SELECT id FROM users LIMIT\n  10",
    "SELECT id FROM users ORDER BY id LIMIT 10",
    "(SELECT id FROM users ORDER BY id) LIMIT 5",
    "((SELECT id FROM users)) LIMIT 5",
    "SELECT id FROM users UNION SELECT id FROM archived LIMIT 3",
    "SELECT * FROM users WHERE EXISTS (SELECT 1 FROM t LIMIT 1)",
    "SELECT * FROM users WHERE NOT EXISTS ((SELECT 1 FROM t LIMIT 1))",
    "SELECT * FROM users WHERE EXISTS (SELECT 1 FROM t WHERE x IN (SELECT y FROM z LIMIT 1))",
    "SELECT (SELECT name FROM t LIMIT 1) FROM users",
    "SELECT id FROM users, LATERAL (SELECT * FROM t LIMIT 1) l",
    "WITH c AS (SELECT id FROM users LIMIT 5) SELECT id FROM c",
    "UPDATE users SET name = (SELECT name FROM t LIMIT 1)",
    "DELETE FROM users WHERE id IN (SELECT id FROM t LIMIT 10) RETURNING *",
    "INSERT INTO users (id) SELECT id FROM t LIMIT 1 RETURNING *",
    "SELECT 'é', * FROM users LIMIT 1",
    "SELECT id\nFROM users\nLIMIT 1;\nDELETE\n  FROM users;",
];

fn restriction_rules() -> RuleConfig {
    let mut rules = RuleConfig::default();
    rules
        .configure("restriction", RuleLevel::Warn)
        .expect("restriction is a category");
    rules
}

fn analyze(catalog: &Catalog, sql: &str, inhouse: bool) -> Vec<Diagnostic> {
    parser_hook::set_enabled(inhouse);
    let diagnostics = Analyzer::with_dialect(catalog, SqlDialect::PostgreSQL)
        .with_rules(restriction_rules())
        .analyze(sql);
    parser_hook::set_enabled(false);
    diagnostics
}

fn sql_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sql_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "sql") {
            out.push(path);
        }
    }
}

#[test]
fn checks_on_the_tree_match_their_sqlparser_versions() {
    let catalog = Catalog::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![];
    sql_files(&root.join("tests/fixtures"), &mut files);
    sql_files(&root.join("examples"), &mut files);
    let file_texts: Vec<(String, String)> = files
        .iter()
        .filter_map(|f| Some((f.display().to_string(), std::fs::read_to_string(f).ok()?)))
        .collect();
    let cases = RESTRICTIONS
        .iter()
        .map(|sql| ((*sql).to_string(), (*sql).to_string()))
        .chain(file_texts);

    let mut reported = 0;
    for (name, sql) in cases {
        let theirs = analyze(&catalog, &sql, false);
        let ours = analyze(&catalog, &sql, true);
        let restriction =
            |d: &&Diagnostic| matches!(d.code(), "E0024" | "E0025" | "E0026" | "E0028");
        let theirs: Vec<_> = theirs.iter().filter(restriction).collect();
        let ours: Vec<_> = ours.iter().filter(restriction).collect();
        assert_eq!(
            format!("{ours:#?}"),
            format!("{theirs:#?}"),
            "restriction diagnostics differ for {name}"
        );
        reported += ours.len();
    }
    assert!(reported > 30, "only {reported} restriction diagnostics");
}
