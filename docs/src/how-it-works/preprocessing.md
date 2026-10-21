# Preprocessing and locations

sqlparser reads SQL and nothing else. Yet sqlsift checks psql scripts with `\set` and `:var`, sqlc queries with `sqlc.arg(id)`, dbt models full of `{{ }}` and `{% %}`, and SQL inside TypeScript, Python, Go and Rust files, and it reports each problem at the right line and column of the original file. It does this without a source map.

## Rewrite, but keep every line and column

Every preprocessing step turns its input into SQL that sqlparser accepts **without moving anything**: what isn't SQL becomes spaces, and what needs replacing is replaced by text no longer than the original, padded with spaces. Line breaks are always kept.

```text
psql        \set x 1⏎SELECT 1                  →  "        ⏎SELECT 1"
psql        x = :'v' AND y = :w                →  "x = $1   AND y = $1"
TypeScript  const q = sql`SELECT 1`;           →  "             ;SELECT 1; "
TypeScript  sql`… a = ${x.y} AND b IN ${ids}`  →  "   ;… a = $1     AND b IN ($1)  ;"
Jinja       SELECT {{ x }} AS a                →  "SELECT $1      AS a"
Jinja       FROM {{ ref('a') }}                →  "FROM ___ref__a_____"
```

Since the rewritten text has the same lines and columns as the original, a location sqlparser reports in it *is* a location in the original file. The steps don't need to know about each other or carry position maps along, and the parser and the analyzer don't know preprocessing exists. The tests of each step check that the output has as many characters and lines as the input.

Locations are kept as lines and character columns. The psql and sqlc steps also keep byte lengths; the others can't (a Python `\n` escape becomes one space, and non-ASCII text in comments is common), so byte offsets are recomputed from line and column on the original text. The language server converts columns to the UTF-16 units LSP uses.

Two rewrites can't keep lengths, and handle it themselves: the BigQuery rewrite inserts characters and maps diagnostics back over them, and MySQL `INSERT ... SET` is turned into `INSERT ... VALUES` on the parsed syntax tree instead of in the text.

## The steps, in order

`Analyzer::analyze_text` (`analyzer/mod.rs`) runs them one after the other:

1. **Jinja** (`templating.rs`, with `--templating jinja`): comments and statements become spaces; of an `{% if %}` only the first branch that may be true is kept (`is_incremental()` is false, `var()` takes its default, `target.type` is the dialect's adapter); a `{% for %}` body is checked once, as its last iteration; `ref()` and `source()` become relation names.
2. **aiosql / HugSQL** (`named_queries.rs`): `:id`, `:v*:ids` and friends become placeholders.
3. **psql or MySQL** (`psql.rs`, `mysql.rs`): psql meta-commands, variables and `COPY ... FROM STDIN` data; MySQL versioned comments, index hints and `DEFINER=` clauses.
4. **sqlc** (`sqlc.rs`): `sqlc.arg(name)`, `sqlc.narg(name)`, `sqlc.slice(name)` and, in PostgreSQL, `@name`.
5. **BigQuery** syntax sqlparser doesn't parse.

Jinja goes first so that a `:` or `@` inside `{{ }}` isn't mistaken for a parameter later. Every step leaves string literals, quoted identifiers and comments alone, and knows the operators that look like parameters (`::` casts, `:=`, `a[i:j]` slices, `@>` and `<@`).

## Embedded SQL: extract, then the same path

For TypeScript / JavaScript (`embedded.rs`), Python (`python.rs`), Go (`go.rs`) and Rust (`sqlx.rs`), an extraction step runs first. It turns the whole file into SQL text with the same lines and columns: everything outside the extracted strings becomes spaces, and the quotes or backticks around each string become `;`, so each string is a statement of its own. The result is a `.sql` file that happens to be mostly blank, and it goes through the steps above like any other.

Each language has a small lexer that knows its comments, strings, template literals, raw strings and so on, so it doesn't mistake one for code. None of them parses the language fully, which keeps them small and adds no dependencies.

Deciding which strings are SQL matters more than extracting them: guessing would report errors on strings that aren't SQL at all. So extraction is strict:

| Language | A string is extracted when it is ... |
|---|---|
| TypeScript / JavaScript | a tagged template whose tag is configured (`sql`, `$queryRaw`, ...) |
| Python | the first argument of a configured call (`execute`, `fetchrow`, `text`, ...), as a plain string literal; f-strings and strings built with `+`, `%` or `.format()` are skipped |
| Go | the argument in the query position of a configured call (`QueryContext(ctx, query, ...)`), as a literal or literals joined with `+` |
| Rust | the SQL argument of a sqlx macro (`query!`, `query_as!(T, ...)`) or of a `sqlx::` function |

and in every language only if it starts with a statement keyword (`SELECT`, `WITH`, `INSERT`, ...). Driver placeholders (`%s`, `%(name)s`, `:name`, `?`, `$1`, `@name`) all become untyped parameters.

## Replaced text is where sqlsift stays quiet

A replaced piece of text could have been anything, so diagnostics about it can't be trusted. Each step returns where it replaced what, and the analyzer uses that in three ways:

| Replaced | Example | Becomes | Effect |
|---|---|---|---|
| a value | `:id`, `${x}`, `%s`, `{{ x }}` | `$1` or `?` | untyped, so it never causes a type mismatch |
| a name | `FROM :"tbl"`, `FROM ${table}`, `FROM {{ ref('a') }}` | an identifier of the same length | missing-table and missing-column diagnostics at that place are dropped |
| a clause | `${where}`, `{% if %}...`, HugSQL `:snip:x` | spaces | the statement may have joins the text doesn't show, so columns it can't resolve aren't reported |

For example, in TypeScript:

```ts
const a = await sql`SELECT nme FROM users WHERE id = ${id}`;     // E0002: 'nme' (did you mean 'name'?)
const b = await sql`SELECT * FROM ${sql(table)} WHERE bogus = 1`; // nothing: the table is unknown
const c = await sql`SELECT titel FROM posts ${where}`;            // nothing: ${where} may join another table
```

The last one is a real typo that goes unreported. sqlsift accepts that: `${where}` could be `JOIN comments c ON ...`, and then `titel` might be a column of `comments`.
