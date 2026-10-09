# Roadmap

sqlsift is at v0.1.x. The next release, v0.2, is about adoption rather than new analysis: anyone with a common stack should get sqlsift running on their project in five minutes, and see it catch the query a migration breaks. Progress is tracked in the [v0.2](https://github.com/yukikotani231/sqlsift/milestone/1) and [v0.3](https://github.com/yukikotani231/sqlsift/milestone/2) milestones.

Three principles guide what goes in:

- **Trustworthy**: no false positives. Anything sqlsift can't work out is left unreported.
- **Quick to adopt**: one command and a working example for each common stack.
- **Wider reach**: more places SQL lives (dbt, Rust, Python) and more dialects, once the basics are proven.

## v0.2: prove it can be adopted

| Item | Issue |
| --- | --- |
| `sqlsift init`: detect migrations, dialect and queries, write `sqlsift.toml`, offer a baseline | [#156](https://github.com/yukikotani231/sqlsift/issues/156) |
| Example projects checked in CI: Prisma TypedSQL, sqlx `query_file!`, aiosql, migration review | [#157](https://github.com/yukikotani231/sqlsift/issues/157) |
| aiosql / HugSQL query names in diagnostics | [#158](https://github.com/yukikotani231/sqlsift/issues/158) |
| GitHub Action: report only the diagnostics a pull request introduces | [#159](https://github.com/yukikotani231/sqlsift/issues/159) |
| Name the migration file in rename/drop hints | [#138](https://github.com/yukikotani231/sqlsift/issues/138) |
| Docs no longer saying dbt/Jinja and TypeScript are unsupported | [#131](https://github.com/yukikotani231/sqlsift/issues/131) |

## v0.3: widen reach

Picked by demand after v0.2, roughly in this order:

| Item | Issue |
| --- | --- |
| Baseline strict mode and maintenance tooling | [#137](https://github.com/yukikotani231/sqlsift/issues/137) |
| Fully resolve schema-qualified names | [#160](https://github.com/yukikotani231/sqlsift/issues/160) |
| SQL in Rust code (sqlx `query!` / `query_as!`) | [#161](https://github.com/yukikotani231/sqlsift/issues/161) |
| dbt stage 2: resolve `ref()` / `source()` columns from `target/catalog.json` | [#95](https://github.com/yukikotani231/sqlsift/issues/95) |
| SQL in Python code (configurable call names) | [#162](https://github.com/yukikotani231/sqlsift/issues/162) |

## v1.0 and later

v1.0 is a promise of stability: configuration keys, rule codes and the JSON / SARIF output stay compatible. It comes once sqlsift is used by real teams. Ideas beyond that, started when there is demand:

- Snowflake and BigQuery dialects (most dbt projects run on them)
- Analysing function bodies (PL/pgSQL, `LANGUAGE sql`), e.g. Supabase RPC functions
- More `suspicious` and `pedantic` rules (`= NULL`, `NOT IN` over a nullable column, `UPDATE` / `DELETE` without `WHERE`)
- Editor features in the LSP: completion, hover with column types, go to definition
- Custom rule plugins

## Contributing

Issues labelled [good first issue](https://github.com/yukikotani231/sqlsift/labels/good%20first%20issue) are kept for new contributors and aren't tied to a release; pick any of them whenever you like. For larger items above, comment on the issue first so we can agree on the approach.
