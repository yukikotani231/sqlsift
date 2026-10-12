# sqlsift-parser (experimental)

An in-house PostgreSQL parser for sqlsift ([#257](https://github.com/yukikotani231/sqlsift/issues/257)).
Not used by sqlsift yet, and not published.

- **Lossless tree.** Every token is kept, comments and whitespace included, with exact
  byte ranges. The tree's text is always the input.
- **Arena.** The tree lives in an `Arena` passed to `parse`: each node's children are one
  slice there, and `Arena::reset` frees a file's tree at once and keeps the memory for the
  next file.
- **Error recovery.** The tree is always built. A syntax error becomes an `Error` node and
  the parser continues at the next clause or statement; a statement the grammar doesn't
  cover yet is an `OpaqueStmt`.
- **Lowering.** `lower::lower_statement` turns a statement into sqlparser's AST, so the
  analyzer can run on this parser unchanged. A construct the lowering doesn't handle is
  `Unsupported`, never a guess. This is a bridge: the analyzer should read the new tree
  directly later.

## Differential testing

`diff` parses each statement with both parsers and compares the ASTs, first by structure,
then with spans. `tests/differential.rs` requires no difference on the repository's SQL.
For other SQL (the corpus in `tests/corpus/`):

```bash
cargo run -p sqlsift-parser --bin sqlsift-parser-diff -- [-v] [-e] <files or directories>
```

It prints a count per outcome (`same`, `spans differ`, `different`, `unsupported`,
`only ours`, `neither`), the unsupported reasons, and every statement whose AST differs.

## Running sqlsift on it

`analysis::statements` splits a file into statements and lowers each one. The CLI built
with the `inhouse-parser` feature reads PostgreSQL files (queries and schema) with it when
`SQLSIFT_PARSER=inhouse` is set (`sqlsift_core::parser_hook`): lowered statements are
analyzed as they are, the others are parsed by sqlparser as before, and a statement this
parser reads but sqlparser can't is skipped without a parse error.

```bash
cargo build --release -p sqlsift-cli --features inhouse-parser
python3 tests/corpus/run.py <projects>                          # sqlparser
SQLSIFT_PARSER=inhouse python3 tests/corpus/run.py <projects>   # this parser
```

Comparing the two `out/results/` is the go / no-go check of #257: no new false positive,
and at least the same detections.

## Status

Statements: `SELECT` (`WITH [RECURSIVE]`, set operations, `VALUES`, joins, subqueries,
`LATERAL`, table functions, `WINDOW`, locking clauses), `INSERT` / `UPDATE` / `DELETE`
(`ON CONFLICT`, `RETURNING`), `CREATE TABLE`, `CREATE [MATERIALIZED] VIEW`, `CREATE TYPE`,
`CREATE INDEX`, `ALTER TABLE`, `DROP`. Expressions: operators with PostgreSQL precedence,
`IS`, `IN`, `BETWEEN`, `LIKE` / `ILIKE` / `SIMILAR TO`, `ANY` / `ALL`, `CASE`, `CAST`,
`::`, `EXISTS`, `ARRAY`, subscripts and field selection, window functions with `FILTER` /
`WITHIN GROUP`, `EXTRACT`, `substring(... FROM ...)` and the other SQL-standard forms,
typed literals, `COLLATE`, `AT TIME ZONE`, all literal kinds. Everything else is an
`OpaqueStmt`.

Next: the analyzer reading the tree directly, one check at a time.
