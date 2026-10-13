# Command line

```
sqlsift [OPTIONS] <COMMAND>

Commands:
  init      Detect the project layout and write sqlsift.toml
  check     Check SQL files against schema definitions
  impact    Show the queries a schema change breaks or fixes
  describe  Describe what each query returns: its output columns and their types
  unused    List the tables and columns that no query uses
  rules     List all rules with their category and default level
  schema    Display the schema sqlsift loaded (tables, views, enum types)
  parse     Parse SQL and display AST (for debugging)

Global options:
  -v, --verbose   Enable verbose logging to stderr (-vv for debug)
  -q, --quiet     Suppress summary/non-error output
  -h, --help      Print help
  -V, --version   Print version
```

## `sqlsift init`

```
sqlsift init [OPTIONS] [DIR]

Arguments:
  [DIR]                  Project directory [default: current directory]

Options:
      --force            Overwrite an existing sqlsift.toml
  -d, --dialect <NAME>   SQL dialect to write instead of the detected one
      --no-check         Don't run a first check after writing sqlsift.toml
      --dry-run          Print the configuration to stdout instead of writing sqlsift.toml
```

Looks at the project, writes a commented `sqlsift.toml` in `DIR`, runs a first `sqlsift check` with it and prints a summary (diagnostics per rule) and the next steps. It never asks questions, so it also works in scripts. An existing `sqlsift.toml` is left alone unless `--force` is given. Directories of dependencies and build output (`node_modules`, `target`, `vendor`, `dist`, ...) and hidden directories are not searched.

| Found | Written |
|-------|---------|
| `sqlc.yaml` / `sqlc.yml` / `sqlc.json` | `schema` / `schema_dir` and `files` from its `schema` and `queries`, `dialect` from `engine` |
| `prisma/migrations/`, `schema.prisma` | `schema_dir = "prisma/migrations"`, `dialect` from the datasource `provider` |
| `prisma/sql/` (TypedSQL) | `files` |
| `supabase/migrations/` | `schema_dir`, `dialect = "postgresql"` |
| Drizzle output (`drizzle/`, `out` of `drizzle.config.*`) | `schema_dir`, `dialect` from the config's `dialect` |
| Flyway (`V1__name.sql`), golang-migrate / sqlx (`*.up.sql`), goose, dbmate or plain SQL migrations in a `migrations/` / `migration/` / `migrate/` directory | `schema_dir` |
| `db/structure.sql` (Rails), `priv/repo/structure.sql` (Ecto), `schema.sql`, `*_schema.sql` dumps, `schema/` directories of DDL | `schema` / `schema_dir`; for Rails, `dialect` from `config/database.yml` |
| `dbt_project.yml` | `files` for the models (`model-paths`); the dialect from `profiles.yml` in the project or the `dbt-<adapter>` requirement |
| Other `.sql` files | `files`, grouped by directory and kept clear of the schema files (with `ignore` where needed) |
| `.ts` / `.js` / `.vue` / `.svelte` files with `` sql`...` `` or Prisma `` $queryRaw`...` `` templates | `files`, and `embedded_sql_tags` for `$queryRaw` / `$executeRaw` |
| `.rs` files with sqlx `query!("...")` / `query_as!(T, "...")` macros or `sqlx::query("...")` calls | `files` |
| `.py` files passing SQL to `cur.execute("...")`, `conn.fetch("...")`, `text("...")` and the other [default calls](../guide/queries.md#sql-in-python) | `files` |
| `.go` files passing SQL to `db.QueryContext(ctx, "...")`, `db.Get(&u, "...")` and the other [default calls](../guide/queries.md#sql-in-go) | `files` |

When several schema sources are found, the most specific one is used and the others are written as comments: a tool's own layout first, then the source creating the most tables, then the shallowest. Sources under test data (`test/`, `fixtures/`, `examples/`, `integration_test/`, ...) come last. A directory with one subdirectory per database (`migrations/mysql/`, `migrations/postgres/`, `db/h2/`) is one source per database, and the one for the project's dialect is preferred. The dialect comes from the most specific source: the tool's own configuration first, then `DATABASE_URL` in `.env.example` / `.env.sample` / `.env`, database images in `docker-compose.yml`, database drivers in `package.json`, `go.mod`, `Gemfile`, Python requirements, `Cargo.toml` or `mix.exs`, and finally the directory or the syntax of the schema files (comments aside); otherwise PostgreSQL. Disagreeing hints are printed. When the first check reports many diagnostics, `init` suggests [a baseline](../guide/suppression.md#an-existing-backlog-baseline).

Exit codes: `0` when the configuration was written (whatever the first check found), `2` when `sqlsift.toml` already exists (without `--force`) or the options are invalid.

## `sqlsift check`

```
sqlsift check [OPTIONS] [FILES]...

Arguments:
  [FILES]...                SQL, TypeScript or JavaScript files to check (glob patterns supported; `-` reads stdin).
                            Defaults to `files` in sqlsift.toml.

Options:
  -s, --schema <FILE>       Schema definition file (repeatable)
      --schema-dir <DIR>    Directory containing schema files
      --ignore <PATTERN>    Skip query files matching a glob pattern (repeatable)
  -c, --config <FILE>       Path to configuration file [default: sqlsift.toml in the
                            current or a parent directory]
  -A, --allow <RULE>        Turn a rule or category off (alias: --disable)
  -W, --warn <RULE>         Report a rule or category as warnings
  -D, --deny <RULE>         Report a rule or category as errors
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks [default: postgresql]
      --templating <ENGINE> Query file templating: jinja (dbt models), none [default: jinja
                            when dbt_project.yml is in the current or the config file's
                            directory or above the query file, else none]
      --dbt-catalog <PATH>  dbt catalog.json with the columns of the models and sources
                            that ref() / source() name (alpha) [default:
                            target/catalog.json of the dbt project, if it exists]
  -f, --format <FORMAT>     Output format: human, json, sarif, github [default: human]
      --max-errors <N>      Maximum number of errors before stopping [default: 100, 0 = unlimited]
      --max-warnings <N>    Fail (exit 1) when more than N warnings are reported
      --baseline <PATH>     Baseline file of known diagnostics, which are not reported
      --write-baseline      Write every current diagnostic of the checked files to the
                            baseline file and exit 0, keeping the entries of other files
                            that still exist (--baseline, `baseline` in sqlsift.toml, or
                            sqlsift-baseline.json)
      --baseline-rule <RULE>
                            With --write-baseline: only rewrite the entries of these
                            rules (codes or names, repeatable), keeping the others
      --prune-baseline      Remove the baseline entries that no longer occur and exit 0;
                            never adds entries
      --strict-baseline     Fail (exit 1) when baseline entries no longer occur
      --max-baseline-entries <N>
                            Fail (exit 1) when the baseline has more than N entries
      --stdin-filename <PATH>
                            File name to report for the query read from stdin (`-`)
```

Command-line options override `sqlsift.toml`. `-A`, `-W` and `-D` accept a rule code (`E0006`), a rule name (`ambiguous-column`) or a category (`suspicious`), and can be repeated.

Exit codes: `0` no errors, `1` errors reported (or more warnings than `--max-warnings`, stale entries with `--strict-baseline`, a baseline larger than `--max-baseline-entries`), `2` usage or configuration error (including a missing or invalid baseline file). Diagnostics in the baseline don't count; see [Baseline](../guide/suppression.md#an-existing-backlog-baseline).

## `sqlsift baseline stats`

```
sqlsift baseline stats [--baseline <PATH>] [-c <FILE>] [--format human|json]
```

Counts the entries of the baseline file (`--baseline`, `baseline` in `sqlsift.toml`, or `sqlsift-baseline.json`) by rule and by file, files with the most entries first:

```text
sqlsift-baseline.json: 153 entries in 12 file(s)

By rule:
  140  E0002 column-not-found
   13  E0008 missing-required-column

By file:
   61  queries/reports/monthly.sql
   ...
```

`--format json` prints `{"baseline", "version", "entries", "rules": [{"code", "name", "entries"}], "files": [{"file", "entries"}]}`.

## `sqlsift impact`

```
sqlsift impact [OPTIONS] [MIGRATIONS]...

Arguments:
  [MIGRATIONS]...           Migration (schema) files whose effect is shown (glob patterns
                            supported). Required unless --base is given.

Options:
      --base <REV>          Also the schema files added or changed since the current branch
                            left this git revision (e.g. origin/main), uncommitted and
                            untracked files included
      --references          Also list the lines that use a changed table or view without a
                            diagnostic changing (where to look when reviewing the change)
      --queries <PATTERN>   Query files to check (repeatable, glob patterns supported)
                            [default: `files` in sqlsift.toml]
  -s, --schema <FILE>       Schema definition file (repeatable)
      --schema-dir <DIR>    Directory containing schema files
      --ignore <PATTERN>    Skip query files matching a glob pattern (repeatable)
  -c, --config <FILE>       Path to configuration file
  -A, --allow <RULE>        Turn a rule or category off (alias: --disable)
  -W, --warn <RULE>         Report a rule or category as warnings
  -D, --deny <RULE>         Report a rule or category as errors
  -d, --dialect <NAME>      SQL dialect [default: postgresql]
      --templating <ENGINE> Query file templating: jinja, none
      --dbt-catalog <PATH>  dbt catalog.json (alpha)
  -f, --format <FORMAT>     Output format: human, json, sarif, github [default: human]
```

`impact` builds the schema twice, without and with the migrations, checks the query files against both and reports only the diagnostics that differ: those the change introduces and those it fixes. Problems the queries already had are not shown.

- A migration that is one of the schema files is left out of the "before" schema together with every schema file after it (files are applied in order: `--schema` files first, then `--schema-dir` files in version order). A migration that isn't one of them is applied after all schema files.
- Only query files that name a changed table or view are checked, so the run stays fast on large projects. Changes that can affect any query (enum types, the search path, functions the schema defines) check every file.
- A diagnostic whose message changes but stays at the same place (a different type in a type mismatch) counts as the same problem.

```console
$ sqlsift impact --base origin/main
Impact of migrations/0042_rename_email.sql
  changed table public.users

error[E0002]: Column 'email' not found in table 'users'
  --> queries/users.sql:12:12
   ...
    = help: 'email' was renamed to 'email_address' in migrations/0042_rename_email.sql:1

The change introduces 1 problem(s) in 1 file(s) and fixes 0
Checked 3 of 41 query file(s); the others name no changed table or view
```

With `--format json` the output is `{"migrations": [...], "changes": [{"name", "kind", "relation"}], "other_changes": bool, "introduced": [...], "resolved": [...]}`, where `introduced` and `resolved` list files and diagnostics as [`check --format json`](output-formats.md) does. `sarif` and `github` report what the change introduces.

With `--references`, the JSON output also has `references`: `[{"file", "lines": [{"line", "uses": ["public.users", "public.users.email", ...]}]}]`.

Exit codes: `0` the change introduces no errors, `1` it does, `2` usage or configuration error.

## `sqlsift unused`

Lists the tables and columns that no query uses, as candidates for cleanup:

```
$ sqlsift unused
audit_log       never referenced
users.token     never referenced
users.seen_at   written (2 statement(s)), never read
```

It checks every query file (`files` in `sqlsift.toml`, or the files given on the command line) and the schema's own views, then lists the tables they never reference, followed by the columns of the other tables they never read. A column is "written, never read" when only INSERT column lists, INSERT without a column list or UPDATE SET use it. `SELECT *` (also through a view) reads every column, and the columns of primary keys, unique constraints, foreign keys (either side) and CHECK constraints count as used.

The queries sqlsift sees are not always all the queries of a database: an ORM, another service or a BI tool may use what is listed. Exclude those with `--exclude <PATTERN>` or `unused_exclude` in `sqlsift.toml`, glob patterns of `table`, `schema.table`, `table.column` or `schema.table.column` (`--exclude 'audit_*' --exclude 'users.legacy_*'`). The report says when it is incomplete: query files with statements that could not be parsed, views in the schema that could not be parsed, and functions or procedures, whose bodies are not analyzed.

| Option | Description |
|--------|-------------|
| `[FILES]...` | Query files (glob patterns supported); default `files` in `sqlsift.toml` |
| `-s, --schema <FILE>` / `--schema-dir <DIR>` | Schema, as for `check` |
| `--exclude <PATTERN>` | Never report tables or columns matching the pattern (repeatable) |
| `--fail-on-unused` | Exit `1` when anything is listed |
| `--ignore`, `-d, --dialect`, `--templating`, `--dbt-catalog`, `-c, --config` | As for `check` |
| `-f, --format <FORMAT>` | `human` (default) or `json` |

The JSON output is `{"unused": [{"table", "column", "status", "writes"}], "files", "incomplete": [...]}`, where `status` is `unreferenced` or `written` and a whole table has no `column`.

Exit codes: `0` (also when something is listed, unless `--fail-on-unused`), `1` something is listed with `--fail-on-unused`, `2` usage or configuration error.

## `sqlsift describe`

```
sqlsift describe [OPTIONS] [FILES]...

Arguments:
  [FILES]...                SQL, TypeScript or JavaScript files to describe (glob patterns
                            supported) [default: `files` in sqlsift.toml]

Options:
  -s, --schema <FILE>       Schema definition file (repeatable)
      --schema-dir <DIR>    Directory containing schema files
      --ignore <PATTERN>    Skip query files matching a glob pattern (repeatable)
  -c, --config <FILE>       Path to configuration file
  -d, --dialect <NAME>      SQL dialect [default: postgresql]
      --templating <ENGINE> Query file templating: jinja, none
      --dbt-catalog <PATH>  dbt catalog.json (alpha)
  -f, --format <FORMAT>     Output format: human, json [default: human]
```

`describe` lists every SELECT, INSERT, UPDATE and DELETE of the query files with what it returns: its output columns, their types and whether they can be NULL, and the types of its parameters. Queries are found as `check` finds them (sqlc / aiosql / HugSQL query names, SQL in tagged templates, dbt models), from the schema alone, without a database. Use it to generate types for raw SQL, or to see what a query returns.

```console
$ sqlsift describe queries/users.sql
queries/users.sql:2 GetUser (select)
  parameters: $1 text
  id           bigint not null
  name         text null
  order_count  bigint not null
```

for

```sql
-- name: GetUser :one
SELECT u.id, u.name, count(o.id) AS order_count
FROM users u LEFT JOIN orders o ON o.user_id = u.id
WHERE u.email = $1
GROUP BY u.id;
```

sqlsift never guesses: a name, type or NULL-ability it can't tell is `null` in JSON (`?` or nothing in the listing).

- **Columns**: `null` (`columns unknown`) when they can't be determined, such as `SELECT *` over an unknown table or a dbt model whose template tags may add columns. INSERT, UPDATE and DELETE return their RETURNING list, or no columns.
- **Names**: an unaliased expression is named as PostgreSQL names it (`count(*)` is `count`, unquoted names are lowercase). In other dialects, and for expressions PostgreSQL calls `?column?`, the name is `null`.
- **`nullable`**: `false` only when the column certainly can't be NULL: a NOT NULL or primary key column that is not on the optional side of an outer join, a literal, `count(...)`, an `IS NULL` test, an operator or function over such values. Aggregates other than `count`, scalar subqueries, CASE without ELSE and ROLLUP / CUBE rows are `true`.
- **Parameters** (`$1`, `?`, `:name`, `@name`, `sqlc.arg(name)`, `${...}` in tagged templates): typed after what they are compared with, inserted into or assigned to, a cast (`$1::date`), IN lists, BETWEEN, LIKE, `= ANY($1)` (an array) and LIMIT / OFFSET. A parameter used where different types are expected has type `null`. Named parameters keep their name and are numbered in order of first appearance.

With `--format json` the output is an array with one object per query:

```json
[
  {
    "file": "queries/users.sql",
    "line": 2,
    "column": 1,
    "name": "GetUser",
    "kind": "select",
    "columns": [
      { "name": "id", "type": "bigint", "nullable": false },
      { "name": "name", "type": "text", "nullable": true },
      { "name": "order_count", "type": "bigint", "nullable": false }
    ],
    "parameters": [{ "index": 1, "name": null, "type": "text" }]
  }
]
```

`kind` is `select`, `insert`, `update` or `delete`. Types are written as the dialect names them.

Exit codes: `0` the files have no errors, `1` they do (their columns may be unknown or wrong; run `check` to see the errors), `2` usage or configuration error.

## `sqlsift schema`

```
sqlsift schema [OPTIONS] [FILES]...

Arguments:
  [FILES]...                Schema definition files (same as --schema; glob patterns supported)

Options:
  -s, --schema <FILE>       Schema definition file (repeatable)
      --schema-dir <DIR>    Directory containing schema files
  -c, --config <FILE>       Path to configuration file
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite, snowflake, bigquery, redshift, databricks [default: postgresql]
  -f, --format <FORMAT>     Output format: human, json [default: human]
```

Without schema arguments, the schema settings from `sqlsift.toml` are used. See [Loading your schema](../guide/schema.md#inspecting-the-loaded-schema).

## `sqlsift rules`

Prints every rule with its code, name, category, default level and description. See [Rules and levels](../guide/rules.md).

## `sqlsift parse`

```
sqlsift parse <FILE>
```

Prints the parsed syntax tree of a file. Useful when reporting a parse problem.
