# Real-world corpus

A harness that checks sqlsift against public dbt projects and warehouse SQL
(`manifest.toml`) to find false positives and measure false negatives. Nothing is
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
