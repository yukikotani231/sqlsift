# Testing and the real-world corpus

A static analyzer is judged by two numbers: how often it reports valid SQL (false positives) and how often it misses broken SQL (false negatives). Hand-written tests can only cover the SQL their authors thought of, so sqlsift is also checked against SQL that other people run in production, with a real database deciding who is right.

## Four layers

| Layer | Where | Runs |
|---|---|---|
| Unit tests and doc tests | `#[cfg(test)]` modules, examples in doc comments | every CI run |
| Integration tests | `crates/*/tests/` | every CI run |
| Example projects | `examples/`, `scripts/check-examples.sh` | every CI run |
| Real-world corpus | `tests/corpus/` | by hand |

### Unit and integration tests

Most tests have the same shape: a schema, a query, and the diagnostics expected, often none. Tests that assert that valid SQL produces no diagnostics are the main defense against false positives, and there are many of them, per dialect and per input format. There are no snapshot tests: each test says what it checks.

Some tests enforce design rules rather than features. The preprocessing tests check that every rewrite keeps the number of characters and lines ([Preprocessing and locations](preprocessing.md)). A registry test checks that the rule list is in the order of the `DiagnosticKind` variants. A robustness test analyzes a 3,000-term `OR` chain on a small thread stack.

The CLI tests run the built `sqlsift` binary in a temporary directory and check exit codes and output formats. The language server tests start `sqlsift-lsp` as a child process and talk JSON-RPC to it over stdio, as an editor would.

### Example projects

Each project in [`examples/`](../integrations/examples.md) mirrors a common setup (sqlc, Prisma TypedSQL, postgres.js, psycopg, aiosql, Go `database/sql`, sqlx, dbt, plain migrations, MySQL, SQLite). CI checks two things for each:

1. `sqlsift check` with the project's `sqlsift.toml` reports nothing, warnings included;
2. the queries in `broken/` report exactly the diagnostics listed in `expected-diagnostics.txt`, by file, line, column and code.

So the examples the documentation points to can't silently stop working.

## The corpus

`tests/corpus/` is a harness (Python) that runs sqlsift on public projects listed in `manifest.toml`: dbt projects (dbt-labs examples, dbt-utils, Fivetran packages and others), warehouse SQL (mozilla/bigquery-etl, Snowflake and Redshift samples), and applications using sqlc, Prisma, postgres.js, Supabase and MySQL. Nothing is vendored: the repositories are cloned when the harness runs, and only projects with a license are used.

### PostgreSQL decides

The harness doesn't rely on anyone's judgment of whether a diagnostic is right:

- dbt projects are actually built on a local PostgreSQL (`dbt seed`, `dbt run`, `dbt docs generate`). This gives a real `catalog.json` with every model's columns, and the SQL dbt compiled. That SQL ran, so **any diagnostic sqlsift reports on it, other than a parse error, is a false positive**.
- For applications, each diagnostic is checked with `EXPLAIN` on PostgreSQL (`EXPLAIN (GENERIC_PLAN)` for queries with `$1` parameters) and classified as a real bug, a sqlsift false positive, or a gap in the harness (for example, a schema that isn't in the repository).

The dbt models are checked four ways: the compiled SQL, the Jinja templates with `--dbt-catalog` (how dbt users run sqlsift), the templates with `ref()` already resolved by the harness, and the raw templates.

### Mutation testing for missed errors

False positives show up on correct code. Missed errors need broken code, so `mutate.py` breaks correct models in realistic ways: a misspelled column, a misspelled CTE name, a self join that makes columns ambiguous, a comparison of a non-date column with a `DATE`. Each mutant is compiled by dbt, and only those PostgreSQL rejects (`EXPLAIN` fails) count as broken. A mutant is detected when sqlsift reports something on it that it doesn't report on the original model.

### Results

Measured in October 2026 on the compiled SQL of 848 dbt models:

| | |
|---|---|
| False positives | 0 |
| Mutants detected | 99.8% (1,603 of 1,606) |

Splitting misses by cause matters more than the totals: when detection was at 94%, 90 of the 93 misses had a single cause (a column that a `SELECT *` CTE returned twice was not reported as ambiguous), so fixing one thing removed almost all of them.

### From the corpus back to CI

The corpus needs dbt, PostgreSQL and time, so it isn't part of CI. Instead, every class of false positive it finds becomes a minimal reproduction in `crates/sqlsift-core/tests/corpus_regression_tests.rs`, with a comment naming the project it came from. Application queries are rewritten with tables and columns of our own rather than copied. A last test checks that the kinds of breakage the mutation testing injects are still found. The heavy run stays manual, and what it taught stays in CI.

## Adding a test with a change

- A new rule: queries where it must report, and several valid ways of writing similar SQL where it must not, per dialect when the dialects differ.
- A false positive fix: the smallest schema and query that showed it, asserting no diagnostics.
- A new input format: the diagnostics with their exact lines and columns in the original file.
