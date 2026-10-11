# Prisma TypedSQL (PostgreSQL)

A [Prisma](https://www.prisma.io) project using [TypedSQL](https://www.prisma.io/docs/orm/prisma-client/using-raw-sql/typedsql) files in `prisma/sql/` and raw queries (`$queryRaw`, `$executeRaw`) in TypeScript. sqlsift builds the schema from the SQL migrations Prisma Migrate writes, so `schema.prisma` itself isn't read.

```text
sqlsift.toml
prisma/schema.prisma
prisma/migrations/20260101120000_init/migration.sql
prisma/migrations/20260215093000_add_view_count/migration.sql
prisma/sql/getTopPosts.sql            TypedSQL
prisma/sql/getUsersWithPostCount.sql
src/posts.ts                          prisma.$queryRawTyped / $queryRaw / $executeRaw
broken/sql/getDraftsByAuthor.sql      intentional mistakes
broken/src/admin.ts
```

`sqlsift.toml`:

```toml
schema_dir = "prisma/migrations"
files = ["prisma/sql/*.sql", "src/**/*.ts"]
dialect = "postgresql"
embedded_sql_tags = ["sql", "$queryRaw", "$executeRaw"]
```

`embedded_sql_tags` makes sqlsift check the SQL in `` prisma.$queryRaw`...` `` and `` prisma.$executeRaw`...` `` templates; `${...}` values are placeholders, and `Prisma.sql` fragments are skipped. TypedSQL's `$1` parameters and `-- @param` comments need no setup.

## Run it

```console
$ sqlsift check
All 3 file(s) passed validation
```

## What it catches

Prisma's quoted, camelCase table and column names are easy to get wrong in raw SQL, and TypedSQL only finds out when `prisma generate --sql` runs against a database:

```console
$ sqlsift check broken/sql/getDraftsByAuthor.sql broken/src/admin.ts
error[E0002]: Column 'updatedAt' not found in table 'Post'
  --> broken/sql/getDraftsByAuthor.sql:3:19
    |
  3 | SELECT id, title, "updatedAt"
    |                   ^^^^^^^^^^^
    = help: Did you mean 'createdAt'?

error[E0003]: Invalid value 'OWNER' for enum type 'Role'
  --> broken/src/admin.ts:8:23
    |
  8 |     UPDATE "User" SET role = 'OWNER' WHERE id = ${userId}
    |                       ^^^^
    = help: Valid values: 'USER', 'ADMIN'

error[E0001]: Table 'Users' not found
  --> broken/src/admin.ts:16:10
    |
 16 |     FROM "Users"
    |          ^^^^^^^
    = help: Did you mean 'User'?

Found 3 error(s), 0 warning(s) in 2 file(s)
```

CI checks both commands on every change to sqlsift (`scripts/check-examples.sh`, against `expected-diagnostics.txt`).
