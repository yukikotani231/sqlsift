//! End-to-end tests for the `sqlsift-lsp` server.
//!
//! Each test spawns the real binary and drives it over stdio using JSON-RPC
//! with `Content-Length` framing, exactly like an editor would.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);
const USERS_SCHEMA: &str = "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);

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
            "sqlsift-lsp-cov-{prefix}-{}-{n}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create temp dir");
        let path = path.canonicalize().expect("canonicalize temp dir");
        // Windows canonical paths carry a `\\?\` prefix that file URIs can't express
        let path = match path.to_string_lossy().strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => path,
        };
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        fs::write(&p, content).expect("failed to write file");
        p
    }

    fn uri(&self, rel: &str) -> String {
        file_uri(&self.path.join(rel))
    }

    fn root_uri(&self) -> String {
        file_uri(&self.path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn file_uri(p: &Path) -> String {
    // Proper file URIs on every platform (`file:///C:/...` on Windows)
    tower_lsp::lsp_types::Url::from_file_path(p)
        .expect("absolute path")
        .to_string()
}

/// A running language server with a background reader thread.
struct Lsp {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    next_id: i64,
    /// Messages received but not yet consumed by a matcher.
    backlog: Vec<Value>,
}

impl Lsp {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sqlsift-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env_remove("RUST_LOG")
            .spawn()
            .expect("failed to spawn sqlsift-lsp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut content_length: Option<usize> = None;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length:") {
                        content_length = v.trim().parse().ok();
                    }
                }
                let Some(len) = content_length else { return };
                let mut buf = vec![0u8; len];
                if reader.read_exact(&mut buf).is_err() {
                    return;
                }
                let Ok(msg) = serde_json::from_slice::<Value>(&buf) else {
                    return;
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            rx,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    fn send(&mut self, msg: &Value) {
        let body = serde_json::to_string(msg).expect("serialize");
        let stdin = self.stdin.as_mut().expect("stdin open");
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).expect("write");
        stdin.flush().expect("flush");
    }

    /// Build a JSON-RPC message; `params` is omitted entirely when null
    /// (tower-lsp rejects an explicit `"params": null` for `shutdown`).
    fn message(id: Option<i64>, method: &str, params: Value) -> Value {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if let Some(id) = id {
            msg["id"] = json!(id);
        }
        if !params.is_null() {
            msg["params"] = params;
        }
        msg
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&Self::message(None, method, params));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&Self::message(Some(id), method, params));
        self.wait_for(|m| m.get("id") == Some(&json!(id)) && m.get("method").is_none())
    }

    /// Wait for the first message satisfying `pred`, keeping others in the backlog.
    fn wait_for(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(pos) = self.backlog.iter().position(&pred) {
            return self.backlog.remove(pos);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    if pred(&msg) {
                        return msg;
                    }
                    self.backlog.push(msg);
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!(
                        "timed out waiting for message; backlog: {:#?}",
                        self.backlog
                    )
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("server closed stdout; backlog: {:#?}", self.backlog)
                }
            }
        }
    }

    /// Wait for the next publishDiagnostics notification for `uri`.
    fn diagnostics_for(&mut self, uri: &str) -> Vec<Value> {
        let uri = uri.to_string();
        let msg = self.wait_for(|m| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri
        });
        msg["params"]["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .clone()
    }

    fn initialize(&mut self, root_uri: Option<&str>) -> Value {
        let root = root_uri.map_or(Value::Null, |u| json!(u));
        let resp = self.request(
            "initialize",
            json!({"processId": null, "rootUri": root, "capabilities": {}}),
        );
        self.notify("initialized", json!({}));
        resp
    }

    /// Initialize and wait for the "initialized" log message; returns it.
    fn start(&mut self, root_uri: Option<&str>) -> String {
        self.initialize(root_uri);
        let log = self.wait_for(|m| {
            m["method"] == "window/logMessage"
                && m["params"]["message"]
                    .as_str()
                    .is_some_and(|s| s.contains("sqlsift LSP initialized"))
        });
        log["params"]["message"].as_str().unwrap().to_string()
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.open_as(uri, "sql", text);
    }

    fn open_as(&mut self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": language_id, "version": 1, "text": text}}),
        );
    }

    fn change(&mut self, uri: &str, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}]
            }),
        );
    }

    fn shutdown_and_exit(mut self) -> Option<i32> {
        let resp = self.request("shutdown", Value::Null);
        assert!(resp.get("error").is_none(), "shutdown error: {resp}");
        self.notify("exit", Value::Null);
        drop(self.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.code();
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("server did not exit after shutdown/exit");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Temp workspace with `schema.sql` + `sqlsift.toml` pointing at it.
fn workspace(prefix: &str, extra_toml: &str) -> TempDir {
    let t = TempDir::new(prefix);
    t.write("schema.sql", USERS_SCHEMA);
    t.write(
        "sqlsift.toml",
        &format!("schema = [\"schema.sql\"]\n{extra_toml}"),
    );
    t
}

fn codes(diags: &[Value]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[test]
fn initialize_advertises_capabilities() {
    let t = workspace("caps", "");
    let mut lsp = Lsp::spawn();
    let resp = lsp.initialize(Some(&t.root_uri()));
    let caps = &resp["result"]["capabilities"];
    let sync = &caps["textDocumentSync"];
    assert_eq!(sync["openClose"], true);
    assert_eq!(sync["change"], 1, "FULL sync expected");
    assert_eq!(sync["save"]["includeText"], true);
    assert_eq!(caps["hoverProvider"], true);
    let triggers = caps["completionProvider"]["triggerCharacters"]
        .as_array()
        .expect("trigger characters");
    assert!(triggers.contains(&json!(".")));
}

#[test]
fn initialized_logs_schema_file_count() {
    let t = workspace("init-log", "");
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&t.root_uri()));
    assert!(msg.contains("(1 schema file(s) loaded)"), "{msg}");
}

#[test]
fn shutdown_then_exit_terminates_cleanly() {
    let t = workspace("shutdown", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    assert_eq!(lsp.shutdown_and_exit(), Some(0));
}

#[test]
fn request_before_initialize_is_rejected() {
    let mut lsp = Lsp::spawn();
    let resp = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": "file:///x.sql"}, "position": {"line": 0, "character": 0}}),
    );
    assert!(resp.get("error").is_some(), "expected error: {resp}");
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[test]
fn did_open_publishes_diagnostics_with_zero_indexed_range() {
    let t = workspace("open-diag", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT 1;\n\nSELECT\n  nme FROM users;\n");
    let diags = lsp.diagnostics_for(&uri);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    let d = &diags[0];
    assert_eq!(d["code"], "E0002");
    assert_eq!(d["source"], "sqlsift");
    assert_eq!(d["severity"], 1);
    assert_eq!(d["range"]["start"], json!({"line": 3, "character": 2}));
    assert_eq!(d["range"]["end"], json!({"line": 3, "character": 5}));
    let message = d["message"].as_str().unwrap();
    assert!(message.starts_with("Column 'nme' not found"), "{message}");
    assert!(message.contains("Help: Did you mean 'name'?"), "{message}");
}

#[test]
fn table_not_found_range() {
    let t = workspace("table-range", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT id FROM orderz;");
    let diags = lsp.diagnostics_for(&uri);
    let d = diags
        .iter()
        .find(|d| d["code"] == "E0001")
        .expect("E0001 diagnostic");
    assert_eq!(d["range"]["start"], json!({"line": 0, "character": 15}));
    assert_eq!(d["range"]["end"], json!({"line": 0, "character": 21}));
}

#[test]
fn valid_document_publishes_empty_diagnostics() {
    let t = workspace("open-valid", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT id, name FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn did_change_fixing_error_clears_diagnostics() {
    let t = workspace("change-fix", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT nme FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0002"]);
    lsp.change(&uri, 2, "SELECT name FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn did_change_introducing_error_publishes_it() {
    let t = workspace("change-break", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT name FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
    lsp.change(&uri, 2, "SELECT name FROM users WHERE id = 'abc';");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0003"]);
}

#[test]
fn did_close_clears_diagnostics() {
    let t = workspace("close", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT nme FROM users;");
    assert!(!lsp.diagnostics_for(&uri).is_empty());
    lsp.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn did_save_with_text_reanalyzes_document() {
    let t = workspace("save-doc", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT id FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
    lsp.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri}, "text": "SELECT zzz FROM users;"}),
    );
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0002"]);
}

#[test]
fn saving_schema_file_rebuilds_catalog_and_reanalyzes_open_documents() {
    let t = workspace("save-schema", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT email FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0002"]);

    t.write(
        "schema.sql",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT);\n",
    );
    lsp.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": t.uri("schema.sql")}}),
    );
    lsp.wait_for(|m| {
        m["method"] == "window/logMessage"
            && m["params"]["message"]
                .as_str()
                .is_some_and(|s| s.contains("Schema updated"))
    });
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn multiple_documents_are_tracked_independently() {
    let t = workspace("multi-doc", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let a = t.uri("a.sql");
    let b = t.uri("b.sql");
    lsp.open(&a, "SELECT nme FROM users;");
    lsp.open(&b, "SELECT id FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&a)), vec!["E0002"]);
    assert!(lsp.diagnostics_for(&b).is_empty());
}

#[test]
fn parse_error_publishes_e1000() {
    let t = workspace("parse-err", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELEC id FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E1000"]);
}

#[test]
fn inline_suppression_is_honored() {
    let t = workspace("suppress", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(
        &uri,
        "-- sqlsift:disable E0002\nSELECT nme FROM users;\nSELECT other FROM users;\n",
    );
    let diags = lsp.diagnostics_for(&uri);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0]["range"]["start"]["line"], 2);
}

#[test]
fn disable_file_directive_is_honored() {
    let t = workspace("disable-file", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(
        &uri,
        "SELECT nme FROM users;\n-- sqlsift:disable-file E0002\nSELECT id FROM nope;\nSELEC broken;\n",
    );
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0001", "E1000"]);
}

#[test]
fn config_ignored_files_get_no_diagnostics() {
    let t = workspace("ignore", "ignore = [\"gen/**\", \"**/*.generated.sql\"]\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let ignored = t.uri("gen/q.sql");
    lsp.open(&ignored, "SELECT nme FROM users;");
    assert!(lsp.diagnostics_for(&ignored).is_empty());
    let generated = t.uri("sub/x.generated.sql");
    lsp.open(&generated, "SELECT nme FROM users;");
    assert!(lsp.diagnostics_for(&generated).is_empty());
    let checked = t.uri("q.sql");
    lsp.open(&checked, "SELECT nme FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&checked)), vec!["E0002"]);
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[test]
fn config_disable_filters_diagnostics() {
    let t = workspace("cfg-disable", "disable = [\"E0002\"]\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(
        &uri,
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'x';",
    );
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0003"]);
}

#[test]
fn config_rule_levels_set_severity() {
    let t = workspace(
        "cfg-rules",
        "[rules]\ncolumn-not-found = \"warn\"\nE0003 = \"off\"\n",
    );
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(
        &uri,
        "SELECT nme FROM users;\nSELECT id FROM users WHERE id = 'x';",
    );
    let diagnostics = lsp.diagnostics_for(&uri);
    assert_eq!(codes(&diagnostics), vec!["E0002"]);
    // LSP DiagnosticSeverity::WARNING
    assert_eq!(diagnostics[0]["severity"], 2, "{diagnostics:?}");
}

#[test]
fn unknown_rule_in_config_is_reported() {
    let t = workspace("bad-rule", "[rules]\nno-such-rule = \"off\"\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let msg = wait_for_warning(&mut lsp, "no-such-rule");
    assert!(msg.contains("unknown rule or category"), "{msg}");
}

#[test]
fn misspelled_rule_in_config_gets_a_suggestion() {
    let t = workspace("typo-rule", "[rules]\nambigous-column = \"off\"\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let msg = wait_for_warning(&mut lsp, "ambigous-column");
    assert!(msg.contains("Did you mean 'ambiguous-column'?"), "{msg}");
}

#[test]
fn config_schema_dir_is_loaded_in_order() {
    let t = TempDir::new("cfg-schema-dir");
    t.write("migrations/001_create.sql", USERS_SCHEMA);
    t.write(
        "migrations/002_alter.sql",
        "ALTER TABLE users ADD COLUMN email TEXT;\n",
    );
    t.write("migrations/notes.txt", "CREATE TABLE ignored (x INT);\n");
    t.write("sqlsift.toml", "schema_dir = \"migrations\"\n");
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&t.root_uri()));
    assert!(msg.contains("(2 schema file(s) loaded)"), "{msg}");
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT email FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn config_schema_glob_patterns_are_expanded() {
    let t = TempDir::new("cfg-glob");
    t.write("db/users.sql", USERS_SCHEMA);
    t.write(
        "db/orders.sql",
        "CREATE TABLE orders (id INTEGER, user_id INTEGER);\n",
    );
    t.write("sqlsift.toml", "schema = [\"db/*.sql\"]\n");
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&t.root_uri()));
    assert!(msg.contains("(2 schema file(s) loaded)"), "{msg}");
    let uri = t.uri("q.sql");
    lsp.open(
        &uri,
        "SELECT o.id FROM orders o JOIN users u ON u.id = o.user_id;",
    );
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn config_dialect_mysql_is_applied() {
    let t = workspace("cfg-mysql", "dialect = \"mysql\"\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT `id` FROM `users`;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn default_dialect_rejects_backticks() {
    let t = workspace("cfg-pg", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT `id` FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E1000"]);
}

#[test]
fn config_found_in_parent_of_root() {
    let t = TempDir::new("cfg-parent");
    let schema = t.write("schema.sql", USERS_SCHEMA);
    t.write(
        "sqlsift.toml",
        &format!("schema = [{:?}]\n", schema.display().to_string()),
    );
    fs::create_dir_all(t.path().join("sub/project")).unwrap();
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&file_uri(&t.path().join("sub/project"))));
    assert!(msg.contains("(1 schema file(s) loaded)"), "{msg}");
}

// ---------------------------------------------------------------------------
// Graceful degradation
// ---------------------------------------------------------------------------

#[test]
fn missing_config_reports_zero_schema_files_and_still_diagnoses() {
    let t = TempDir::new("no-config");
    if t.path()
        .ancestors()
        .any(|a| a.join("sqlsift.toml").exists())
    {
        return;
    }
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&t.root_uri()));
    assert!(msg.contains("(0 schema file(s) loaded)"), "{msg}");
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT id FROM users;");
    assert!(codes(&lsp.diagnostics_for(&uri)).contains(&"E0001".to_string()));
}

#[test]
fn missing_root_uri_is_handled() {
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(None);
    assert!(msg.contains("(0 schema file(s) loaded)"), "{msg}");
    lsp.open("file:///tmp/sqlsift-untitled.sql", "SELECT 1;");
    assert!(lsp
        .diagnostics_for("file:///tmp/sqlsift-untitled.sql")
        .is_empty());
    assert_eq!(lsp.shutdown_and_exit(), Some(0));
}

#[test]
fn config_pointing_at_missing_schema_file_does_not_crash() {
    let t = TempDir::new("missing-schema");
    t.write("sqlsift.toml", "schema = [\"nope.sql\"]\n");
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&t.root_uri()));
    assert!(msg.contains("(0 schema file(s) loaded)"), "{msg}");
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT 1;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn invalid_toml_does_not_crash_server() {
    let t = TempDir::new("bad-toml");
    t.write("schema.sql", USERS_SCHEMA);
    t.write("sqlsift.toml", "schema = [\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT 1;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
    assert_eq!(lsp.shutdown_and_exit(), Some(0));
}

#[test]
fn schema_with_unparseable_statements_still_loads_the_rest() {
    let t = TempDir::new("resilient");
    t.write(
        "schema.sql",
        "CREATE TABLE users (id INTEGER, name TEXT);\nTHIS IS NOT SQL;\nCREATE TABLE orders (id INTEGER);\n",
    );
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT u.name, o.id FROM users u, orders o;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

// ---------------------------------------------------------------------------
// Hover & completion
// ---------------------------------------------------------------------------

#[test]
fn hover_on_table_shows_columns() {
    let t = workspace("hover", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT id FROM users;");
    let resp = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 17}}),
    );
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_else(|| panic!("hover: {resp}"));
    assert!(value.contains("**users** (table)"), "{value}");
    assert!(value.contains("| name | text | NOT NULL |"), "{value}");
}

#[test]
fn hover_on_unknown_word_returns_null() {
    let t = workspace("hover-null", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT 1 AS whatever;");
    let resp = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 14}}),
    );
    assert!(resp["result"].is_null(), "{resp}");
}

#[test]
fn completion_lists_tables_and_columns() {
    let t = workspace("completion", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    lsp.open(&uri, "SELECT ");
    let resp = lsp.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 7}}),
    );
    let items = resp["result"].as_array().expect("completion array");
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    for l in ["users", "id", "name"] {
        assert!(labels.contains(&l), "missing {l} in {labels:?}");
    }
}

#[test]
fn completion_without_schema_returns_null() {
    let mut lsp = Lsp::spawn();
    lsp.start(None);
    let resp = lsp.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": "file:///tmp/x.sql"}, "position": {"line": 0, "character": 0}}),
    );
    assert!(resp["result"].is_null(), "{resp}");
}

#[test]
fn schema_warnings_are_logged_as_warning_messages() {
    let t = TempDir::new("schema-warn");
    t.write(
        "schema.sql",
        "CREATE TABLE users (id INTEGER);\nALTER TABLE ghost ADD COLUMN x INT;\n",
    );
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let m = lsp.wait_for(|m| m["method"] == "window/logMessage" && m["params"]["type"] == 2);
    let text = m["params"]["message"].as_str().unwrap();
    assert!(text.contains("Schema warning"), "{text}");
    assert!(text.contains("ghost"), "{text}");
}

#[test]
fn percent_encoded_root_uri_with_spaces() {
    let t = TempDir::new("space dir");
    t.write("schema.sql", USERS_SCHEMA);
    t.write("sqlsift.toml", "schema = [\"schema.sql\"]\n");
    let mut lsp = Lsp::spawn();
    let root = t.root_uri().replace(' ', "%20");
    let msg = lsp.start(Some(&root));
    assert!(msg.contains("(1 schema file(s) loaded)"), "{msg}");
    let uri = format!("{root}/q.sql");
    lsp.open(&uri, "SELECT nme FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), vec!["E0002"]);
}

// ---------------------------------------------------------------------------
// Regression tests for previously-found LSP bugs
// ---------------------------------------------------------------------------

/// Wait for a warning-level window/showMessage or window/logMessage containing `needle`.
fn wait_for_warning(lsp: &mut Lsp, needle: &str) -> String {
    let needle = needle.to_string();
    let msg = lsp.wait_for(|m| {
        (m["method"] == "window/showMessage" || m["method"] == "window/logMessage")
            && m["params"]["type"] == 2
            && m["params"]["message"]
                .as_str()
                .is_some_and(|s| s.contains(&needle))
    });
    msg["params"]["message"].as_str().unwrap().to_string()
}

#[test]
fn invalid_toml_is_reported_to_the_user() {
    let t = TempDir::new("bad-toml-warn");
    t.write("sqlsift.toml", "schema = [\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let msg = wait_for_warning(&mut lsp, "sqlsift.toml");
    assert!(msg.contains("Failed to parse"), "{msg}");
}

#[test]
fn invalid_dialect_in_config_is_reported() {
    let t = workspace("bad-dialect", "dialect = \"oracle\"\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let msg = wait_for_warning(&mut lsp, "oracle");
    assert!(msg.contains("Unknown dialect"), "{msg}");
}

#[test]
fn relative_schema_paths_resolve_against_config_dir() {
    let t = TempDir::new("cfg-rel-parent");
    t.write("db/schema.sql", USERS_SCHEMA);
    t.write("sqlsift.toml", "schema = [\"db/schema.sql\"]\n");
    fs::create_dir_all(t.path().join("sub/project")).unwrap();
    let mut lsp = Lsp::spawn();
    let msg = lsp.start(Some(&file_uri(&t.path().join("sub/project"))));
    assert!(msg.contains("(1 schema file(s) loaded)"), "{msg}");
    let uri = t.uri("sub/project/q.sql");
    lsp.open(&uri, "SELECT id FROM users;");
    assert!(lsp.diagnostics_for(&uri).is_empty());
}

#[test]
fn ranges_use_utf16_code_units() {
    let t = workspace("utf16", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("q.sql");
    // U+1F600 is 2 UTF-16 code units; 'é' is 1.
    lsp.open(&uri, "SELECT '\u{1F600}é', nme FROM users;");
    let diags = lsp.diagnostics_for(&uri);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 0, "character": 14})
    );
    assert_eq!(
        diags[0]["range"]["end"],
        json!({"line": 0, "character": 17})
    );
}

// ---------------------------------------------------------------------------
// Baseline (#97)
// ---------------------------------------------------------------------------

/// A baseline file with one E0002 entry for `file`, in the statement `statement`
fn baseline_json(file: &str, statement: &str) -> String {
    json!({
        "version": 1,
        "entries": [{
            "file": file,
            "code": "E0002",
            "statement_hash": sqlsift_core::baseline::statement_hash(statement),
            "occurrence": 0,
            "line": 1,
            "message": "Column 'nme' not found in table 'users'"
        }]
    })
    .to_string()
}

#[test]
fn baselined_diagnostics_are_hidden() {
    let t = workspace("baseline", "baseline = \"ci/baseline.json\"\n");
    t.write(
        "ci/baseline.json",
        &baseline_json("../sql/q.sql", "SELECT nme FROM users;"),
    );
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("sql/q.sql");
    // The known problem moved down a line; the new one is still shown
    lsp.open(
        &uri,
        "SELECT id FROM users;\nSELECT nme FROM users;\nSELECT bad FROM users;\n",
    );
    let diagnostics = lsp.diagnostics_for(&uri);
    assert_eq!(codes(&diagnostics), vec!["E0002"]);
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("'bad'"),
        "{diagnostics:?}"
    );
    // Another file with the same statement isn't covered
    let other = t.uri("sql/other.sql");
    lsp.open(&other, "SELECT nme FROM users;");
    assert_eq!(codes(&lsp.diagnostics_for(&other)), vec!["E0002"]);
}

#[test]
fn missing_baseline_file_is_reported() {
    let t = workspace("baseline-missing", "baseline = \"nope.json\"\n");
    let mut lsp = Lsp::spawn();
    lsp.initialize(Some(&t.root_uri()));
    let msg = wait_for_warning(&mut lsp, "nope.json");
    assert!(msg.contains("Failed to read baseline"), "{msg}");
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript and dbt documents
// ---------------------------------------------------------------------------

#[test]
fn typescript_document_checks_tagged_templates() {
    let t = workspace("ts", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("src/users.ts");
    let text = "import { sql } from './db';\n\
                export const q = (id: number) =>\n  sql`SELECT nme FROM users WHERE id = ${id}`;\n";
    lsp.open_as(&uri, "typescript", text);
    let diags = lsp.diagnostics_for(&uri);
    assert_eq!(codes(&diags), ["E0002"], "{diags:#?}");
    assert_eq!(
        diags[0]["range"]["start"],
        json!({"line": 2, "character": 13})
    );
    assert_eq!(
        diags[0]["range"]["end"],
        json!({"line": 2, "character": 16})
    );
}

#[test]
fn typescript_document_without_sql_has_no_diagnostics() {
    let t = workspace("ts-plain", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    for (file, language) in [
        ("src/app.tsx", "typescriptreact"),
        ("src/app.js", "javascript"),
        ("src/app.jsx", "javascriptreact"),
    ] {
        let uri = t.uri(file);
        let text = "const greeting = `hello ${name}`;\n\
                    export function f(a, b) { return a / b; } // SELECT * FROM nowhere\n";
        lsp.open_as(&uri, language, text);
        let diags = lsp.diagnostics_for(&uri);
        assert!(diags.is_empty(), "{file}: {diags:#?}");
    }
}

#[test]
fn typescript_document_is_detected_by_extension() {
    let t = workspace("ts-ext", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("src/q.mts");
    lsp.open_as(&uri, "plaintext", "await sql`SELECT * FROM userz`;\n");
    assert_eq!(codes(&lsp.diagnostics_for(&uri)), ["E0001"]);
}

#[test]
fn config_embedded_sql_tags_are_used() {
    let t = workspace("ts-tags", "embedded_sql_tags = [\"$queryRaw\"]\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("src/q.ts");
    let text = "await prisma.$queryRaw`SELECT nme FROM users`;\n\
                await sql`SELECT bogus FROM users`;\n";
    lsp.open_as(&uri, "typescript", text);
    let diags = lsp.diagnostics_for(&uri);
    assert_eq!(codes(&diags), ["E0002"], "{diags:#?}");
    assert_eq!(diags[0]["range"]["start"]["line"], 0);
}

#[test]
fn completion_and_hover_are_off_in_typescript_documents() {
    let t = workspace("ts-completion", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("src/q.ts");
    lsp.open_as(&uri, "typescript", "const users = 1;\n");
    let resp = lsp.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 6}}),
    );
    assert!(resp["result"].is_null(), "{resp}");
    let resp = lsp.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 8}}),
    );
    assert!(resp["result"].is_null(), "{resp}");
}

const DBT_MODEL: &str = "{{ config(materialized='view') }}\n\
                         SELECT u.id FROM users u JOIN {{ ref('s') }} s ON s.id = u.id\n";

#[test]
fn dbt_project_in_subdirectory_enables_jinja() {
    let t = workspace("dbt-nested", "");
    t.write("analytics/dbt_project.yml", "name: analytics\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));

    let model = t.uri("analytics/models/staging/m.sql");
    lsp.open(&model, DBT_MODEL);
    let diags = lsp.diagnostics_for(&model);
    assert!(diags.is_empty(), "{diags:#?}");

    // Outside the dbt project the template is a parse error
    let other = t.uri("queries/q.sql");
    lsp.open(&other, DBT_MODEL);
    assert!(codes(&lsp.diagnostics_for(&other)).contains(&"E1000".to_string()));
}

#[test]
fn jinja_sql_language_enables_jinja() {
    let t = workspace("jinja-sql", "");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("models/m.sql");
    lsp.open_as(&uri, "jinja-sql", DBT_MODEL);
    let diags = lsp.diagnostics_for(&uri);
    assert!(diags.is_empty(), "{diags:#?}");
}

#[test]
fn explicit_templating_none_wins_over_dbt_detection() {
    let t = workspace("dbt-none", "templating = \"none\"\n");
    t.write("analytics/dbt_project.yml", "name: analytics\n");
    let mut lsp = Lsp::spawn();
    lsp.start(Some(&t.root_uri()));
    let uri = t.uri("analytics/models/m.sql");
    lsp.open_as(&uri, "jinja-sql", DBT_MODEL);
    assert!(codes(&lsp.diagnostics_for(&uri)).contains(&"E1000".to_string()));
}
