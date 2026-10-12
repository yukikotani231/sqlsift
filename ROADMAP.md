# Roadmap

sqlsift v0.2 is out. v0.3 widens where sqlsift can be used, v0.4 replaces the PostgreSQL parser, and v1.0 promises stable output. Progress is tracked in the [v0.3](https://github.com/yukikotani231/sqlsift/milestone/2), [v0.4](https://github.com/yukikotani231/sqlsift/milestone/3) and [v1.0](https://github.com/yukikotani231/sqlsift/milestone/4) milestones.

Three principles guide what goes in:

- **Trustworthy**: no false positives. Anything sqlsift can't work out is left unreported.
- **Quick to adopt**: one command and a working example for each common stack.
- **Wider reach**: more places SQL lives (dbt, Rust, Python) and more dialects. New dialects start as alpha and are checked against a real-world corpus before they leave it.

## v0.2: prove it can be adopted (released)

`sqlsift init` ([#156](https://github.com/yukikotani231/sqlsift/issues/156)), example projects checked in CI ([#157](https://github.com/yukikotani231/sqlsift/issues/157)), aiosql / HugSQL query names ([#158](https://github.com/yukikotani231/sqlsift/issues/158)), a GitHub Action that reports only the diagnostics a pull request introduces ([#159](https://github.com/yukikotani231/sqlsift/issues/159)), migration file names in rename/drop hints ([#138](https://github.com/yukikotani231/sqlsift/issues/138)) and up-to-date docs ([#131](https://github.com/yukikotani231/sqlsift/issues/131)).

Also in v0.2: rules E0009-E0027, including the first `suspicious` and `restriction` rules ([#199](https://github.com/yukikotani231/sqlsift/issues/199)), and Snowflake, BigQuery, Redshift and Databricks in alpha.

## v0.3: widen reach

| Item | Issue |
| --- | --- |
| Fully resolve schema-qualified names (done) | [#160](https://github.com/yukikotani231/sqlsift/issues/160) |
| SQL in Rust code (sqlx `query!` / `query_as!`) (done) | [#161](https://github.com/yukikotani231/sqlsift/issues/161) |
| SQL in Python code (configurable call names) (done) | [#162](https://github.com/yukikotani231/sqlsift/issues/162) |
| SQL in Go code (`database/sql`, sqlx and pgx calls) | [#311](https://github.com/yukikotani231/sqlsift/issues/311) |
| Baseline strict mode and maintenance tooling (done) | [#137](https://github.com/yukikotani231/sqlsift/issues/137) |
| `sqlsift impact`: show which queries a migration breaks (done) | [#230](https://github.com/yukikotani231/sqlsift/issues/230) |
| `sqlsift describe`: each query's columns and types as JSON (done) | [#231](https://github.com/yukikotani231/sqlsift/issues/231) |
| `sqlsift unused`: tables and columns no query uses (done) | [#265](https://github.com/yukikotani231/sqlsift/issues/265) |
| Suspicious joins: a JOIN that doesn't follow the foreign key (E0030), aggregates multiplied by a one-to-many join (E0031) (done) | [#266](https://github.com/yukikotani231/sqlsift/issues/266), [#294](https://github.com/yukikotani231/sqlsift/issues/294) |
| Newer PostgreSQL and MySQL syntax, and mismatches MySQL / SQLite convert at run time (E0029) | [#279](https://github.com/yukikotani231/sqlsift/issues/279)-[#284](https://github.com/yukikotani231/sqlsift/issues/284) |
| `sqlsift init` in larger repositories, query files in UTF-16 or other encodings (done) | [#275](https://github.com/yukikotani231/sqlsift/issues/275), [#276](https://github.com/yukikotani231/sqlsift/issues/276) |

## v0.4: own PostgreSQL parser

| Item | Issue |
| --- | --- |
| In-house PostgreSQL parser with error recovery and a lossless syntax tree; other dialects stay on sqlparser | [#257](https://github.com/yukikotani231/sqlsift/issues/257) |

Replacing the parser changes diagnostic positions and which statements fail to parse, so it comes before v1.0. It ships only if the real-world corpus gives no new false positives and at least the same detections; otherwise sqlsift stays on sqlparser.

## v1.0 and later

v1.0 is a promise of stability: configuration keys, rule codes and the JSON / SARIF output (including `sqlsift describe`) stay compatible. It comes once sqlsift is used by real teams, and nothing in it is alpha any more:

| Item | Issue |
| --- | --- |
| Data warehouse dialects out of alpha: Snowflake, BigQuery, Redshift, Databricks (alpha released; next: feedback from real projects) | [#187](https://github.com/yukikotani231/sqlsift/issues/187) |
| dbt stage 2 out of alpha: `ref()` / `source()` columns from `target/catalog.json` (alpha released; next: feedback from real projects) | [#95](https://github.com/yukikotani231/sqlsift/issues/95) |

Ideas beyond that, started when there is demand:

- Analysing function bodies (PL/pgSQL, `LANGUAGE sql`), e.g. Supabase RPC functions
- `pedantic` and `style` rules (none yet)
- Editor features in the LSP: completion, hover with column types, go to definition (built on the v0.4 parser's error recovery)
- Custom rule plugins

## Contributing

Issues labelled [good first issue](https://github.com/yukikotani231/sqlsift/labels/good%20first%20issue) are kept for new contributors and aren't tied to a release; pick any of them whenever you like. For larger items above, comment on the issue first so we can agree on the approach.
