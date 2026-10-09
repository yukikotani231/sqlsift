# Output formats

Choose a format with `--format` (`-f`) or `format` in `sqlsift.toml`. The machine-readable formats (`json`, `sarif`, `github`) write diagnostics to stdout and logs and the summary line to stderr, so stdout can be piped or redirected to a file. `human` output goes to stderr.

## `human` (default)

Source excerpts with the problem underlined, plus a hint when sqlsift has one:

```text
error[E0002]: Column 'naem' not found in table 'users'
  --> queries/report.sql:1:10
    |
  1 | SELECT u.naem, o.total
    |          ^^^^
    = help: Did you mean 'name'?
```

Colors are used only when stderr is a terminal and `NO_COLOR` is not set.

## `json`

A single JSON document. Only files with diagnostics are listed; `files` is empty when everything passes.

```json
{
  "files": [
    {
      "file": "queries/fetch.sql",
      "diagnostics": [
        {
          "code": "E0002",
          "kind": "ColumnNotFound",
          "severity": "error",
          "message": "Column 'user_id' not found in table 'users'",
          "help": "Did you mean 'id'?",
          "line": 3,
          "column": 15,
          "span": { "line": 3, "column": 15, "length": 7, "offset": 62 },
          "labels": []
        }
      ]
    }
  ]
}
```

`line` and `column` are 1-indexed (columns count characters); `span.offset` is the 0-indexed byte offset of the same position in the file, and `span.length` is in bytes. `severity` is `error` or `warning`. In [sqlc query files](../guide/queries.md#sqlc-query-files), diagnostics also have a `query_name` field with the name from the query's `-- name:` comment; it is absent otherwise.

## `sarif`

A [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html) log with a single run containing the results for all files and a `tool.driver.rules` entry for every rule. A diagnostic in a named sqlc query has the query name as a `logicalLocations` entry (kind `function`) and at the end of its message text, as in `(in query 'ListPosts')`. Upload it to GitHub Code Scanning as shown in [CI and GitHub Actions](../integrations/ci.md#github-code-scanning-sarif).

## `github`

One [GitHub Actions workflow command](https://docs.github.com/en/actions/reference/workflow-commands-for-github-actions) per diagnostic, which GitHub turns into annotations on the pull request diff:

```text
::error file=queries/fetch.sql,line=3,col=15,endLine=3,endColumn=22,title=E0002 column-not-found::Column 'user_id' not found%0Ahelp: Did you mean 'id'?
::warning file=queries/report.sql,line=6,col=8,endLine=6,endColumn=10,title=E0006 ambiguous-column::Column 'id' is ambiguous
```

Warnings become `::warning`, errors `::error`. Use it when you run sqlsift inside your own job step; the [GitHub Action](../integrations/ci.md) sets up annotations for you.

## Schema JSON

`sqlsift schema --format json` prints the loaded schema:

```jsonc
{
  "dialect": "postgresql",
  "default_schema": "public",
  "schema_files": ["migrations/001_init.sql"],
  "schemas": [{
    "name": "public",
    "tables": [{
      "name": "users",
      "columns": [{ "name": "id", "type": "integer", "nullable": false, "primary_key": true,
                    "identity": null, "auto_increment": false, "default": "nextval(...)" }],
      "primary_key": ["id"],           // or null
      "foreign_keys": [{ "name": null, "columns": ["..."], "references_table": "...", "references_columns": ["..."] }],
      "unique": [["..."]]
    }],
    "views": [{ "name": "user_names", "materialized": false,
                "columns": [{ "name": "id", "type": "integer" }] }]   // type is null when unknown
  }],
  "enums": [{ "name": "mood", "values": ["sad", "ok", "happy"] }]
}
```
