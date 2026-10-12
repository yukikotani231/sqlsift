# sqlsift-parser (experimental)

An in-house PostgreSQL parser for sqlsift ([#257](https://github.com/yukikotani231/sqlsift/issues/257)).
Not used by sqlsift yet, and not published.

- **Lossless tree.** Every token is kept, comments and whitespace included, with exact
  byte ranges. The tree's text is always the input.
- **Error recovery.** The tree is always built. A syntax error becomes an `Error` node and
  the parser continues at the next clause or statement; a statement the grammar doesn't
  cover yet is an `OpaqueStmt`.
- **Lowering.** `lower::lower_statement` turns a statement into sqlparser's AST, so the
  analyzer can run on this parser unchanged. A construct the lowering doesn't handle is
  `Unsupported`, never a guess. This is a bridge: the analyzer should read the new tree
  directly later (that is also where an arena per statement pays off).

## Differential testing

`diff` parses each statement with both parsers and compares the ASTs, first by structure,
then with spans. `tests/differential.rs` requires no difference on the repository's SQL.
For other SQL (the corpus in `tests/corpus/`):

```bash
cargo run -p sqlsift-parser --bin sqlsift-parser-diff -- [-v] [-e] <files or directories>
```

It prints a count per outcome (`same`, `spans differ`, `different`, `unsupported`,
`only ours`, `neither`), the unsupported reasons, and every statement whose AST differs.

## Status

Covered: `SELECT` with target lists, `DISTINCT [ON]`, `FROM` (tables, aliases with column
lists, subqueries, parenthesized joins, every join type), `WHERE`, `GROUP BY`, `HAVING`,
`ORDER BY`, `LIMIT`, `OFFSET`, and expressions (operators with PostgreSQL precedence,
`IS`, `IN`, `BETWEEN`, `LIKE` / `ILIKE` / `SIMILAR TO`, `CASE`, `CAST`, `::`, `EXISTS`,
scalar subqueries, row constructors, function calls, parameters, all literal kinds).

Next: `WITH`, set operations, window functions and `FILTER`, table functions and
`LATERAL`, `ANY` / `ALL`, then `INSERT` / `UPDATE` / `DELETE`.
