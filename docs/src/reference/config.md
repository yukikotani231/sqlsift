# Configuration file

`sqlsift check` and `sqlsift schema` look for `sqlsift.toml` in the current directory and its parents, or use the file given with `--config <FILE>`. The language server reads it from the workspace root. Command-line options override values from the file.

```toml
# Schema
schema = ["db/schema/*.sql"]      # schema files (glob patterns supported)
schema_dir = "db/migrations"      # all .sql files under this directory, in file-name order

# Query files
files = ["queries/**/*.sql"]      # query files to check when none are given on the command line
ignore = ["queries/archive/**", "**/*.generated.sql"]  # query files to skip

dialect = "postgresql"            # postgresql, mysql or sqlite
templating = "jinja"              # jinja (dbt models) or none
format = "human"                  # human, json, sarif or github
max_warnings = 0                  # fail when more than this many warnings are reported
baseline = "sqlsift-baseline.json"  # known diagnostics that are not reported
embedded_sql_tags = ["sql", "$queryRaw"]  # template literal tags checked in .ts/.js files

# Rules
disable = ["E0006"]               # rules to turn off (same as `E0006 = "off"` below)

[rules]                           # per-rule level: "off", "warn" or "error"
E0008 = "warn"                    # by code...
ambiguous-column = "off"          # ...or by name

[categories]                      # level of every rule in a category
correctness = "error"
```

## Keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `schema` | list of paths / globs | `[]` | Schema files, loaded in order |
| `schema_dir` | path | none | Directory of schema files, loaded recursively in file-name order, skipping rollback migrations |
| `files` | list of paths / globs | `[]` | Query files to check when none are given on the command line |
| `ignore` | list of globs | `[]` | Query files to skip. `**` matches any number of directories; a pattern matching a directory skips everything below it. `--ignore` adds to this list |
| `dialect` | string | `"postgresql"` | `postgresql`, `mysql` or `sqlite` |
| `templating` | string | auto | `jinja` masks dbt / Jinja templates in query files, `none` turns that off. When unset, `jinja` is used if a `dbt_project.yml` is in the current directory or next to `sqlsift.toml`. See [dbt and Jinja templates](../guide/queries.md#dbt-and-jinja-templates) |
| `format` | string | `"human"` | `human`, `json`, `sarif` or `github` |
| `max_warnings` | integer | none | Fail when more than this many warnings are reported |
| `baseline` | path | none | Baseline file of known diagnostics, hidden by `sqlsift check` and the language server; `--baseline` overrides it. See [Baseline](../guide/suppression.md#an-existing-backlog-baseline) |
| `embedded_sql_tags` | list of strings | `["sql"]` | Tags of the template literals checked as SQL in TypeScript and JavaScript files, matched against the tag's last identifier (see [SQL in TypeScript and JavaScript](../guide/queries.md#sql-in-typescript-and-javascript)) |
| `disable` | list of rules | `[]` | Rules (codes or names) to turn off |
| `[rules]` | table | | Level per rule: `"off"`, `"warn"` or `"error"` |
| `[categories]` | table | | Level per category: `correctness`, `suspicious`, `pedantic`, `style`, `restriction` |

## Paths

Relative paths and patterns in the file are resolved against the directory containing `sqlsift.toml`, so the same file works from any subdirectory.

## Validation

Unknown keys produce a warning. Invalid `dialect`, `templating` or `format` values, unknown rules or categories and invalid levels are errors (exit code `2`), with a suggestion when the name is close to a valid one.
