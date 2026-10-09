# Command line

```
sqlsift [OPTIONS] <COMMAND>

Commands:
  check   Check SQL files against schema definitions
  rules   List all rules with their category and default level
  schema  Display the schema sqlsift loaded (tables, views, enum types)
  parse   Parse SQL and display AST (for debugging)

Global options:
  -v, --verbose   Enable verbose logging to stderr (-vv for debug)
  -q, --quiet     Suppress summary/non-error output
  -h, --help      Print help
  -V, --version   Print version
```

## `sqlsift check`

```
sqlsift check [OPTIONS] [FILES]...

Arguments:
  [FILES]...                SQL files to check (glob patterns supported; `-` reads stdin).
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
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite [default: postgresql]
      --templating <ENGINE> Query file templating: jinja (dbt models), none [default: jinja
                            when dbt_project.yml is in the current or the config file's
                            directory, else none]
  -f, --format <FORMAT>     Output format: human, json, sarif, github [default: human]
      --max-errors <N>      Maximum number of errors before stopping [default: 100, 0 = unlimited]
      --max-warnings <N>    Fail (exit 1) when more than N warnings are reported
      --baseline <PATH>     Baseline file of known diagnostics, which are not reported
      --write-baseline      Write every current diagnostic to the baseline file
                            (--baseline, `baseline` in sqlsift.toml, or
                            sqlsift-baseline.json) and exit 0
      --stdin-filename <PATH>
                            File name to report for the query read from stdin (`-`)
```

Command-line options override `sqlsift.toml`. `-A`, `-W` and `-D` accept a rule code (`E0006`), a rule name (`ambiguous-column`) or a category (`suspicious`), and can be repeated.

Exit codes: `0` no errors, `1` errors reported (or more warnings than `--max-warnings`), `2` usage or configuration error (including a missing or invalid baseline file). Diagnostics in the baseline don't count; see [Baseline](../guide/suppression.md#an-existing-backlog-baseline).

## `sqlsift schema`

```
sqlsift schema [OPTIONS] [FILES]...

Arguments:
  [FILES]...                Schema definition files (same as --schema; glob patterns supported)

Options:
  -s, --schema <FILE>       Schema definition file (repeatable)
      --schema-dir <DIR>    Directory containing schema files
  -c, --config <FILE>       Path to configuration file
  -d, --dialect <NAME>      SQL dialect: postgresql, mysql, sqlite [default: postgresql]
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
