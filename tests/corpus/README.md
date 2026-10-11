# Real-world corpus

A harness that checks sqlsift against public dbt projects, warehouse SQL and
application repositories (`manifest.toml`) to find false positives and measure false negatives. Nothing is
vendored: the projects are cloned into `repos/` and all output goes to `out/` (both
git-ignored). It is not part of `cargo test` or CI; the minimal reproductions it found
are in `crates/sqlsift-core/tests/corpus_regression_tests.rs`.

Requirements: Python 3.11+ with PyYAML, `git`, and for the dbt projects `dbt-postgres`
(1.9/1.10) plus a PostgreSQL server (`PGHOST`/`PGPORT`/`PGUSER`, trust auth).

```bash
cargo build --release -p sqlsift-cli          # or set SQLSIFT=/path/to/sqlsift
python3 tests/corpus/fetch.py                 # shallow clones, writes out/lock.json
python3 tests/corpus/dbt_build.py             # dbt seed/run/docs generate on PostgreSQL
python3 tests/corpus/run.py                   # sqlsift on every file, out/results/
python3 tests/corpus/mutate.py                # mutation testing, out/mutations/
python3 tests/corpus/report.py > out/report-tables.md
python3 tests/corpus/show.py bigquery-etl raw bigquery E0002 --classes   # triage
python3 tests/corpus/blank_retry.py           # parse rate if Jinja tags at parse errors were blanked
```

All scripts take project names to limit the run (`python3 tests/corpus/run.py dbt_stripe`).

## Application repositories

The `kind = "sql"` projects with a `schema_dir` are applications: sqlc queries
(`-- name:`, `@param`, `sqlc.arg()`), Prisma TypedSQL (`prisma/sql/*.sql`), postgres.js
tagged templates in TypeScript, and MySQL queries. They are checked the way their
developers would run sqlsift: the migrations directory with `--schema-dir` (golang-migrate,
dbmate, goose and sql-migrate down sections are skipped) and the query files, or the
`.ts` files for postgres.js. `schema = "ts-sql"` reads migrations written as `sql\`...\``
templates in TypeScript, and `schema_rewrite` patches a copy of the migrations for
applications that rewrite them at runtime.

Each diagnostic in these projects is triaged as a real bug (confirmed with `EXPLAIN` on
PostgreSQL, `EXPLAIN (GENERIC_PLAN)` for queries with `$n` parameters), a false positive
(fixed in sqlsift, with a minimal rewrite in `corpus_regression_tests.rs`), or a harness
gap (the schema isn't in the repository: excluded in the manifest with a comment). Every
project's license is recorded in the manifest and projects without one are not used.

## Modes

- `raw`: the files as they are, with the project's dialect and Jinja templating. The
  schema comes from the manifest's `schema` (for dbt projects that are built: the
  tables of `catalog.json` as DDL, sources also as `<source>.<table>`).
- `catalog`: dbt models checked with `--dbt-catalog` and no schema files, the way a dbt
  user runs sqlsift. The catalog's PostgreSQL types are renamed to each warehouse's type
  names (`NUMBER`, `INT64`, `character varying`, ...).
- `resolved`: `ref()`/`source()`/`var()` relations replaced by their relation name by the
  harness (an upper bound for catalog support).
- `compiled`: the SQL dbt compiled for PostgreSQL, against `catalog.json`. dbt built these
  models, so any diagnostic other than E1000 is a false positive.

## Mutation testing

`mutate.py` adds mutants of each model to the dbt project (a misspelled column, a
misspelled CTE reference, a self cross join that makes columns ambiguous, a `= DATE`
comparison on a non-date column), compiles them with dbt and runs `EXPLAIN` on
PostgreSQL: a mutant PostgreSQL rejects is a real bug, and sqlsift should report a
diagnostic on it that it doesn't report on the original model.
