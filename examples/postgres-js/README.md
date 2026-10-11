# postgres.js / TypeScript tagged templates (PostgreSQL)

A TypeScript service using [postgres.js](https://github.com/porsager/postgres) `` sql`...` `` templates, with [dbmate](https://github.com/amacneil/dbmate) migrations. The same setup works for Slonik, kysely's `sql`, `@vercel/postgres` and other libraries with a `sql` tag.

```text
sqlsift.toml
db/migrations/20260110080000_create_accounts.sql   -- migrate:up / -- migrate:down
db/migrations/20260112090000_create_projects.sql
src/db.ts
src/projects.ts
src/events.ts
broken/billing.ts                                  intentional mistakes
```

`sqlsift.toml`:

```toml
schema_dir = "db/migrations"
files = ["src/**/*.ts"]
dialect = "postgresql"
```

Only the `-- migrate:up` part of each migration is applied. In the code, `${value}` is a placeholder, conditional fragments (`` ${cond ? sql`AND ...` : sql``} ``) are left out, and postgres.js helpers such as `` INSERT INTO events ${sql(rows, ...)} `` and `` UPDATE projects SET ${sql(patch, ...)} `` are checked without the columns they expand to.

## Run it

```console
$ sqlsift check
All 3 file(s) passed validation
```

## What it catches

```console
$ sqlsift check broken/billing.ts
error[E0002]: Column 'plan_id' not found in table 'accounts'
  --> broken/billing.ts:5:33
    |
  5 |   await sql`UPDATE accounts SET plan_id = ${"pro"} WHERE id = ${accountId}`;
    |                                 ^^^^^^^
    = help: Did you mean 'plan'?

error[E0006]: Column 'id' is ambiguous (found in tables: a, p)
  --> broken/billing.ts:11:12
    |
 11 |     SELECT id, a.email, p.name
    |            ^^
    = help: Qualify the column with a table name: a.id

error[E0008]: INSERT into 'projects' is missing required column 'account_id'
  --> broken/billing.ts:20:25
    |
 20 |   await sql`INSERT INTO projects (name) VALUES (${name})`;
    |                         ^^^^^^^^
    = help: 'account_id' is NOT NULL without a default. Provide a value, or add a DEFAULT to the schema

Found 3 error(s), 0 warning(s) in 1 file(s)
```

Diagnostics point at the line and column in the `.ts` file, so they show up in place in the editor (with the VS Code extension) and as pull request annotations (with the GitHub Action).

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
