//! `sqlsift init` on common project layouts.
//!
//! Each test builds a small project in its own temporary directory, runs
//! `sqlsift init` there and checks the written `sqlsift.toml` (and, where it
//! runs, the first check).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Project {
    path: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Project {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("sqlsift-init-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self {
            path: path.canonicalize().expect("canonicalize temp dir"),
        }
    }

    fn write(&self, rel: &str, content: &str) -> &Self {
        let path = self.path.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, content).expect("write file");
        self
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_in(&self.path, args)
    }

    #[allow(clippy::unused_self)]
    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_sqlsift"))
            .current_dir(cwd)
            .args(args)
            .env("NO_COLOR", "1")
            .env_remove("RUST_LOG")
            .output()
            .expect("run sqlsift");
        Run {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }
    }

    /// Run `sqlsift init` (exit code 0 expected) and return its output
    fn init(&self, args: &[&str]) -> Run {
        let mut all = vec!["init"];
        all.extend_from_slice(args);
        let run = self.run(&all);
        assert_eq!(
            run.code,
            Some(0),
            "init failed:\n{}\n{}",
            run.stdout,
            run.stderr
        );
        run
    }

    fn config(&self) -> String {
        fs::read_to_string(self.path.join("sqlsift.toml")).expect("sqlsift.toml written")
    }

    /// The written configuration, parsed
    fn table(&self) -> toml::Table {
        toml::from_str(&self.config()).expect("sqlsift.toml is valid TOML")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn strings(table: &toml::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn string(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

const USERS: &str = "CREATE TABLE users (id SERIAL PRIMARY KEY, email TEXT NOT NULL);\n";

#[test]
fn prisma_migrations_typed_sql_and_query_raw() {
    let p = Project::new("prisma");
    p.write(
        "prisma/schema.prisma",
        "generator client {\n  provider = \"prisma-client-js\"\n}\n\ndatasource db {\n  provider = \"postgresql\"\n  url      = env(\"DATABASE_URL\")\n}\n",
    )
    .write(
        "prisma/migrations/20240101000000_init/migration.sql",
        "CREATE TABLE \"User\" (\"id\" SERIAL NOT NULL, \"email\" TEXT NOT NULL, CONSTRAINT \"User_pkey\" PRIMARY KEY (\"id\"));\n",
    )
    .write("prisma/migrations/migration_lock.toml", "provider = \"postgresql\"\n")
    .write("prisma/sql/getUser.sql", "SELECT id, email FROM \"User\" WHERE id = $1;\n")
    .write("prisma/sql/broken.sql", "SELECT nickname FROM \"User\";\n")
    .write(
        "src/users.ts",
        "export const all = () => prisma.$queryRaw`SELECT email FROM \"User\"`;\n",
    )
    .write("node_modules/pkg/index.ts", "sql`SELECT * FROM nothing`;\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert_eq!(
        string(&table, "schema_dir").as_deref(),
        Some("prisma/migrations")
    );
    assert_eq!(
        strings(&table, "files"),
        ["prisma/sql/**/*.sql", "src/**/*.ts"]
    );
    assert_eq!(
        strings(&table, "embedded_sql_tags"),
        ["sql", "$queryRaw", "$executeRaw"]
    );
    assert!(run.stdout.contains("Prisma migrations"), "{}", run.stdout);
    assert!(
        run.stdout.contains("provider = \"postgresql\""),
        "{}",
        run.stdout
    );
    // The first check finds the broken query
    assert!(
        run.stdout
            .contains("1 error(s), 0 warning(s) in 1 of 3 file(s)"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("E0002 column-not-found"),
        "{}",
        run.stdout
    );
}

#[test]
fn sqlc_with_golang_migrate_schema() {
    let p = Project::new("sqlc");
    p.write(
        "sqlc.yaml",
        "version: \"2\"\nsql:\n  - engine: \"mysql\"\n    schema: \"db/migrations\"\n    queries: \"db/queries\"\n    gen:\n      go:\n        package: \"db\"\n        out: \"internal/db\"\n",
    )
    .write(
        "db/migrations/000001_users.up.sql",
        "CREATE TABLE users (id INT AUTO_INCREMENT PRIMARY KEY, email VARCHAR(255) NOT NULL);\n",
    )
    .write("db/migrations/000001_users.down.sql", "DROP TABLE users;\n")
    .write(
        "db/queries/users.sql",
        "-- name: GetUser :one\nSELECT id, email FROM users WHERE id = ?;\n",
    )
    .write("go.mod", "module x\n\nrequire github.com/lib/pq v1.10.9\n");

    let run = p.init(&[]);
    let table = p.table();
    // sqlc's engine wins over the Go driver
    assert_eq!(string(&table, "dialect").as_deref(), Some("mysql"));
    assert_eq!(
        string(&table, "schema_dir").as_deref(),
        Some("db/migrations")
    );
    assert_eq!(strings(&table, "files"), ["db/queries/*.sql"]);
    assert!(
        run.stdout.contains("sqlc schema (sqlc.yaml)"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("go.mod"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 1 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn goose_migrations_skip_down_sections() {
    let p = Project::new("goose");
    p.write(
        "migrations/20240101_users.sql",
        "-- +goose Up\n-- +goose StatementBegin\nCREATE TABLE users (id SERIAL PRIMARY KEY, email TEXT NOT NULL);\n-- +goose StatementEnd\n\n-- +goose Down\nDROP TABLE users;\n",
    )
    .write("queries/users.sql", "SELECT id, email FROM users;\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "schema_dir").as_deref(), Some("migrations"));
    assert_eq!(strings(&table, "files"), ["queries/**/*.sql"]);
    assert!(run.stdout.contains("goose migrations"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 1 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn flyway_migrations_inside_the_source_tree() {
    let p = Project::new("flyway");
    p.write(
        "src/main/resources/db/migration/V1__users.sql",
        "CREATE TABLE users (id BIGINT PRIMARY KEY, email TEXT NOT NULL);\n",
    )
    .write(
        "src/main/resources/db/migration/U1__users.sql",
        "DROP TABLE users;\n",
    )
    .write(
        "src/main/resources/sql/find_user.sql",
        "SELECT email FROM users WHERE id = 1;\n",
    );

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(
        string(&table, "schema_dir").as_deref(),
        Some("src/main/resources/db/migration")
    );
    assert_eq!(
        strings(&table, "files"),
        ["src/main/resources/sql/**/*.sql"]
    );
    assert!(run.stdout.contains("Flyway migrations"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 1 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn supabase_migrations_and_seed() {
    let p = Project::new("supabase");
    p.write("supabase/config.toml", "project_id = \"x\"\n")
        .write("supabase/migrations/20240101000000_init.sql", USERS)
        .write(
            "supabase/seed.sql",
            "INSERT INTO users (email) VALUES ('a@example.com');\n",
        );

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert_eq!(
        string(&table, "schema_dir").as_deref(),
        Some("supabase/migrations")
    );
    assert_eq!(strings(&table, "files"), ["supabase/seed.sql"]);
    assert!(run.stdout.contains("Supabase"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 1 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn drizzle_migrations_and_sql_templates() {
    let p = Project::new("drizzle");
    p.write(
        "drizzle.config.ts",
        "import { defineConfig } from 'drizzle-kit';\nexport default defineConfig({\n  schema: './src/schema.ts',\n  out: './drizzle',\n  dialect: 'mysql',\n});\n",
    )
    .write(
        "drizzle/0000_init.sql",
        "CREATE TABLE `users` (\n\t`id` int AUTO_INCREMENT NOT NULL,\n\t`email` varchar(255) NOT NULL,\n\tCONSTRAINT `users_id` PRIMARY KEY(`id`)\n);\n--> statement-breakpoint\nCREATE INDEX `email_idx` ON `users` (`email`);\n",
    )
    .write("drizzle/meta/_journal.json", "{}\n")
    .write(
        "src/queries.ts",
        "import { sql } from 'drizzle-orm';\nexport const q = sql`SELECT email FROM users WHERE id = ${1}`;\n",
    );

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("mysql"));
    assert_eq!(string(&table, "schema_dir").as_deref(), Some("drizzle"));
    assert_eq!(strings(&table, "files"), ["src/**/*.ts"]);
    assert!(table.get("embedded_sql_tags").is_none());
    assert!(run.stdout.contains("Drizzle migrations"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 1 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn sqlx_migrations_and_query_macros() {
    let p = Project::new("sqlx");
    p.write(
        "Cargo.toml",
        "[package]\nname = \"app\"\n\n[dependencies]\nsqlx = { version = \"0.8\", features = [\"postgres\", \"runtime-tokio\"] }\n",
    )
    .write("migrations/20260101000000_create_users.sql", USERS)
    .write(
        "src/db.rs",
        "pub async fn email(pool: &sqlx::PgPool, id: i32) -> sqlx::Result<String> {\n    \
         let row = sqlx::query!(\"SELECT email FROM users WHERE id = $1\", id)\n        \
         .fetch_one(pool)\n        .await?;\n    Ok(row.email)\n}\n",
    )
    .write("src/main.rs", "fn main() {}\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert_eq!(string(&table, "schema_dir").as_deref(), Some("migrations"));
    assert_eq!(strings(&table, "files"), ["src/**/*.rs"]);
    assert!(run.stdout.contains("sqlx query!"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 2 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn python_migrations_and_cursor_queries() {
    let p = Project::new("python");
    p.write("requirements.txt", "psycopg[binary]==3.2.3\n")
        .write("migrations/0001_create_users.sql", USERS)
        .write(
            "app/db.py",
            "def email(cur, user_id):\n    cur.execute(\"SELECT email FROM users WHERE id = %s\", (user_id,))\n    return cur.fetchone()\n",
        )
        .write("app/main.py", "print('hello')\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert_eq!(strings(&table, "files"), ["app/**/*.py"]);
    assert!(run.stdout.contains("cursor.execute"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 2 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn rails_structure_sql_without_queries() {
    let p = Project::new("rails");
    p.write("db/structure.sql", USERS)
        .write(
            "db/migrate/20240101000000_create_users.rb",
            "class CreateUsers; end\n",
        )
        .write(
            "config/database.yml",
            "default: &default\n  adapter: mysql2\n  encoding: utf8mb4\n",
        );

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("mysql"));
    assert_eq!(strings(&table, "schema"), ["db/structure.sql"]);
    assert!(table.get("files").is_none());
    assert!(run.stdout.contains("Rails structure.sql"), "{}", run.stdout);
    assert!(
        run.stdout.contains("Set `files` in sqlsift.toml"),
        "{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("Running a first check"),
        "{}",
        run.stdout
    );
}

#[test]
fn dbt_project_without_catalog() {
    let p = Project::new("dbt");
    p.write(
        "dbt_project.yml",
        "name: shop\nprofile: shop\nmodel-paths: [\"models\"]\n",
    )
    .write(
        "models/staging/stg_orders.sql",
        "select * from {{ source('shop', 'orders') }}\n",
    )
    .write(
        "macros/cents.sql",
        "{% macro cents(x) %}{{ x }} / 100{% endmacro %}\n",
    )
    .write("requirements.txt", "dbt-snowflake==1.8.0\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("snowflake"));
    assert_eq!(strings(&table, "files"), ["models/**/*.sql"]);
    assert!(table.get("schema").is_none() && table.get("schema_dir").is_none());
    assert!(run.stdout.contains("dbt docs generate"), "{}", run.stdout);
    assert!(
        !run.stdout.contains("Running a first check"),
        "{}",
        run.stdout
    );
}

#[test]
fn schema_dump_query_dirs_and_driver_dialect() {
    let p = Project::new("dump");
    p.write("db/schema.sql", USERS)
        .write("db/queries/users.sql", "SELECT id FROM users;\n")
        .write("report.sql", "SELECT count(*) FROM users;\n")
        .write("package.json", "{\"dependencies\": {\"pg\": \"^8.0.0\"}}\n");

    let run = p.init(&[]);
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert_eq!(strings(&table, "schema"), ["db/schema.sql"]);
    assert_eq!(
        strings(&table, "files"),
        ["db/queries/**/*.sql", "report.sql"]
    );
    assert!(run.stdout.contains("package.json: pg"), "{}", run.stdout);
    assert!(
        run.stdout.contains("All 2 file(s) passed"),
        "{}",
        run.stdout
    );
}

#[test]
fn empty_project_writes_a_template() {
    let p = Project::new("empty");
    let run = p.init(&[]);
    let config = p.config();
    let table = p.table();
    assert_eq!(string(&table, "dialect").as_deref(), Some("postgresql"));
    assert!(config.contains("# schema_dir = "), "{config}");
    assert!(config.contains("# files = "), "{config}");
    assert!(run.stdout.contains("not found"), "{}", run.stdout);
    assert!(
        run.stdout.contains("Set `schema` or `schema_dir`"),
        "{}",
        run.stdout
    );
}

#[test]
fn many_diagnostics_suggest_a_baseline() {
    let p = Project::new("baseline");
    p.write("schema.sql", USERS);
    let queries = (0..12).fold(String::new(), |mut sql, i| {
        sql.push_str("SELECT c");
        sql.push_str(&i.to_string());
        sql.push_str(" FROM users;\n");
        sql
    });
    p.write("queries/all.sql", &queries);

    let run = p.init(&[]);
    assert!(run.stdout.contains("12 error(s)"), "{}", run.stdout);
    assert!(run.stdout.contains("--write-baseline"), "{}", run.stdout);
}

#[test]
fn existing_config_needs_force() {
    let p = Project::new("force");
    p.write("schema.sql", USERS)
        .write("queries/q.sql", "SELECT id FROM users;\n")
        .write("sqlsift.toml", "# mine\n");

    let refused = p.run(&["init"]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.stderr.contains("--force"), "{}", refused.stderr);
    assert_eq!(p.config(), "# mine\n");

    // --dry-run prints the configuration and leaves the file alone
    let dry = p.init(&["--dry-run"]);
    assert!(
        dry.stdout.contains("schema = [\"schema.sql\"]"),
        "{}",
        dry.stdout
    );
    assert!(dry.stderr.contains("Detected:"), "{}", dry.stderr);
    assert_eq!(p.config(), "# mine\n");

    p.init(&["--force", "--no-check"]);
    assert_eq!(strings(&p.table(), "schema"), ["schema.sql"]);
}

#[test]
fn dialect_option_and_project_directory_argument() {
    let p = Project::new("dir");
    p.write("app/schema.sql", USERS)
        .write("app/queries/q.sql", "SELECT id FROM users;\n");

    let run = p.run(&["init", "app", "--dialect", "sqlite", "--no-check"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let config = fs::read_to_string(p.path.join("app/sqlsift.toml")).expect("written");
    let table: toml::Table = toml::from_str(&config).expect("valid TOML");
    assert_eq!(string(&table, "dialect").as_deref(), Some("sqlite"));
    assert_eq!(strings(&table, "schema"), ["schema.sql"]);
    assert!(
        run.stdout.contains("sqlsift check` (in app)"),
        "{}",
        run.stdout
    );

    // The written config works from the project directory, without warnings
    let check = p.run_in(&p.path.join("app"), &["check"]);
    assert_eq!(check.code, Some(0), "{}", check.stderr);
    assert!(!check.stderr.contains("Warning"), "{}", check.stderr);

    let bad = p.run(&["init", "app", "--dialect", "oracle", "--force"]);
    assert_eq!(bad.code, Some(2));
    assert!(bad.stderr.contains("Unknown dialect"), "{}", bad.stderr);
}
