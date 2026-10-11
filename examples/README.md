# Example projects

Small, realistic projects showing how to set up sqlsift for common stacks. Each one has a schema (usually migrations), a few queries, a hand-written `sqlsift.toml`, and a `broken/` directory with intentional mistakes and the diagnostics they produce.

| Example | Stack | Shows |
|---|---|---|
| [`sqlc-go`](sqlc-go) | sqlc + golang-migrate, PostgreSQL | `-- name:` queries, `@param` / `sqlc.arg()`, `*.down.sql` skipped |
| [`prisma-typedsql`](prisma-typedsql) | Prisma Migrate + TypedSQL, PostgreSQL | `prisma/sql/*.sql`, `$queryRaw` / `$executeRaw` in TypeScript, quoted camelCase names |
| [`postgres-js`](postgres-js) | postgres.js + dbmate, PostgreSQL | `` sql`...` `` templates in TypeScript, fragments and `sql(...)` helpers |
| [`sqlx`](sqlx) | sqlx `query_file!` + `migrate!()`, PostgreSQL | `queries/*.sql` with one statement each, `<timestamp>_name.sql` migrations, a dropped column caught without a database |
| [`postgres-migrations`](postgres-migrations) | Plain SQL + migrations, PostgreSQL | **A migration renames a column; the query still using the old name is caught** |
| [`mysql`](mysql) | Flyway-style migrations, MySQL | `?` placeholders, inline `ENUM`, `ON DUPLICATE KEY UPDATE` |
| [`dbt-postgres`](dbt-postgres) | dbt on PostgreSQL | Jinja masking, `{{ source() }}` columns, incremental models |

In each directory:

```bash
sqlsift check                       # the project's own queries: clean
sqlsift check broken/<file>...      # the intentional mistakes: fails with the errors shown in the README
```

`broken/` is outside the `files` patterns of `sqlsift.toml`, so `sqlsift check` stays clean; naming files on the command line replaces those patterns.

## Checked in CI

`scripts/check-examples.sh` runs both commands on every example and fails when a project's own queries report anything, or when `broken/` doesn't report exactly the diagnostics in `expected-diagnostics.txt` (`<file>:<line>:<column> <code>` per line). It runs in the repository's CI, so the examples keep working as sqlsift changes.

```bash
cargo build && scripts/check-examples.sh          # all examples
scripts/check-examples.sh sqlc-go mysql           # some of them
```
