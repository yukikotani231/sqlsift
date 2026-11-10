# Name resolution and types

The `Resolver` (`analyzer/resolver.rs`) checks one statement at a time. It looks names up in a `Scope` (`analyzer/scope.rs`), infers the type of each expression (`analyzer/type_check.rs`), and returns the output columns of every query block it visits, with their types.

## Scope: a stack of query blocks

SQL decides which names are visible where by nesting: a subquery in `WHERE` sees the outer query's tables (a correlated subquery), a subquery in `FROM` doesn't (unless it is `LATERAL`), a CTE is visible to the rest of its `WITH` and to the query after it. sqlsift models this as a stack of frames, one per query block:

```text
SELECT u.name                                  frame 1: CTEs of the outer query (none)
FROM users u                                   frame 2: FROM users u
WHERE EXISTS (
  SELECT 1                                     frame 3: CTEs of the subquery (none)
  FROM orders o                                frame 4: FROM orders o
  WHERE o.user_id = u.id                       ← u is found by walking down the stack
)
```

Each query pushes two frames: one for its `WITH` clause, then one for its `FROM` clause. A column is looked up from the innermost frame outwards, and the first frame that has it wins. That is how SQL resolves names too, so an inner `id` shadows an outer one without any special case.

Visibility rules are flags on frames rather than code in each check. A frame pushed for a subquery in `FROM` (not `LATERAL`), or for the `SELECT` of an `INSERT ... SELECT`, has `hides_parent` set, and the lookup skips the frame below it. The checks never walk `FROM` clauses themselves: they ask the scope. This keeps one definition of "what is visible here" for name resolution, type inference and every rule.

### Relations with unknown columns

A relation in a frame is a table, view, CTE, subquery or table function, with its columns as `Option<Vec<Column>>`. `None` means the columns can't be known: a table whose definition couldn't be parsed, a table function sqlsift doesn't know, `SELECT *` over such a table. Looking a column up in it answers *maybe*, not *no*:

| Lookup result | What happens |
|---|---|
| found in exactly one relation | resolved, its type is used |
| found in several relations of the block | E0006 ambiguous column |
| not found, and every relation's columns are known | E0002 column not found |
| not found, but some relation's columns are unknown | nothing is reported |

This is where the "when in doubt, say nothing" rule is implemented for names. An unknown relation never causes a cascade of errors.

### Output columns

Every query block returns its output columns: their names, types and (for `describe`) whether they can be NULL. A CTE or a subquery in `FROM` becomes a relation whose columns are those output columns, so the outer query sees typed columns without walking the inner query again. Views and `CREATE TABLE ... AS SELECT` in the schema get their columns the same way (`analyzer::query_output_columns`).

## Types

Two type representations are involved:

- `SqlType` (`types/mod.rs`) is a column type from the schema: integers of each size, `numeric`, text types, date and time types, `json`, arrays, enums, and so on.
- `ExpressionType` is what an expression evaluates to, with three cases:
  - `Known(SqlType)`
  - `StringLiteral`: a quoted string has no type until it meets the other operand. `'2024-01-01'` compared with a `date` column is a date; compared with an integer column it is an error only if the text can't be a number.
  - `Unknown`: anything that can't be inferred (`NULL`, an unknown function, a parameter, a column of an unknown relation). `Unknown` is never reported.

`SqlType::is_compatible_with` says whether two types are the same, implicitly castable (integer to bigint, integer to numeric) or need an explicit cast. Only the last is a mismatch (E0003, or E0007 in a join condition). Enums, custom types and semi-structured types (`VARIANT`, `JSON` in some warehouses) are treated as compatible with anything, because their conversions depend on things the schema doesn't say.

### Dialects

The same comparison can be an error in one database and fine in another. PostgreSQL rejects `integer = text`; MySQL and SQLite convert the text at run time. sqlsift reports the PostgreSQL case as E0003 and the MySQL / SQLite case as E0029, a warning. Booleans are integers in MySQL and SQLite. These differences are methods of `SqlDialect` (`dialect/`), called from the checks, so a rule doesn't need its own dialect switch.

### What is inferred

Column references, literals, casts, arithmetic and comparisons, `CASE` (the branches must agree), set operations (`UNION` columns must agree), and the return types of common functions (`count`, `sum`, `coalesce`, `lower`, ...). Each new piece of inference makes more mismatches visible, but an expression that isn't covered is `Unknown`, not wrong.

## Checks in the walk

The checks are methods of `Resolver`, grouped by file:

| File | Checks |
|---|---|
| `resolver.rs` | missing tables and columns, ambiguity, missing required columns in `INSERT`, "did you mean" and "was renamed in migration X" hints |
| `type_check.rs` | type mismatches, `NULL` into `NOT NULL` columns |
| `structure.rs` | duplicate names and targets, positions, misplaced aggregates, generated columns, `ON CONFLICT` targets, `DISTINCT` with `ORDER BY`, column counts |
| `grouping.rs`, `ranges.rs`, `signatures.rs` | `GROUP BY` errors (with primary key functional dependency), out-of-range values, function argument types |
| `suspicious.rs`, `fan_out.rs` | the warnings: `= NULL`, `NOT IN` over nullable columns, missing join conditions, outer joins undone by `WHERE`, joins on the wrong key, double counting |
| `restriction.rs` | the opt-in bans |

A check runs where the walk already has what it needs: the `GROUP BY` check runs when a `SELECT` has resolved its select list, the `INSERT` checks when the target table and the values are known. Adding a rule means finding that place, not writing a new pass.
