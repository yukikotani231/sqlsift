# Rules and levels

Every diagnostic comes from a rule. Each rule has a code (`E0002`), a name (`column-not-found`) and a category. You can refer to a rule by either its code or its name anywhere sqlsift takes a rule.

`sqlsift rules` lists them:

```console
$ sqlsift rules
CODE   NAME                      CATEGORY     DEFAULT  DESCRIPTION
E0001  table-not-found           correctness  error    Referenced table does not exist in schema
E0002  column-not-found          correctness  error    Referenced column does not exist in table
E0003  type-mismatch             correctness  error    Type incompatibility in expression
E0004  potential-null-violation  correctness  error    Potential NOT NULL violation
E0005  column-count-mismatch     correctness  error    Column count doesn't match (INSERT, column aliases, subqueries)
E0006  ambiguous-column          correctness  error    Column reference is ambiguous across tables
E0007  join-type-mismatch        correctness  error    JOIN condition compares incompatible types
E0008  missing-required-column   correctness  error    INSERT omits a NOT NULL column without a default
E0009  duplicate-name            correctness  error    Table alias or CTE name given twice in one query
E0010  duplicate-target-column   correctness  error    Column given twice in an INSERT column list or UPDATE SET
E0011  position-out-of-range     correctness  error    ORDER BY / GROUP BY position is not in the select list
E0012  misplaced-aggregate       correctness  error    Aggregate or window function where it is not allowed
E0013  generated-column-write    correctness  error    INSERT or UPDATE gives a value to a generated column
E0014  unmatched-conflict-target correctness  error    ON CONFLICT columns match no unique constraint
E0015  distinct-order-by         correctness  error    SELECT DISTINCT ordered by a column it doesn't select
E0016  grouping-error            correctness  error    Column must appear in GROUP BY or be used in an aggregate
E0017  value-out-of-range        correctness  error    Literal too long or out of range for the column type
E0018  null-comparison           suspicious   warn     Comparison with NULL using = or <> is never true
E0019  not-in-with-nulls         suspicious   warn     NOT IN over a nullable subquery column
E0020  outer-column-in-subquery  suspicious   warn     IN subquery selects a column of the outer query
E0021  missing-join-condition    suspicious   warn     Tables in FROM with no condition linking them
E0022  constant-condition        suspicious   warn     Condition that is always or never true from the schema
E0023  outer-join-filtered       suspicious   warn     WHERE condition turns an outer join into an inner join
E0024  unfiltered-write          restriction  off      UPDATE or DELETE without WHERE
E0025  insert-without-columns    restriction  off      INSERT without a column list
E0026  select-star               restriction  off      SELECT * in a query's result columns
E0027  wrong-argument-type       correctness  error    Function or operator given an argument type it doesn't take
E0028  limit-without-order-by    restriction  off      LIMIT or OFFSET without ORDER BY
E0029  implicit-conversion       suspicious   warn     Type mismatch MySQL or SQLite converts implicitly
E0030  foreign-key-join-mismatch suspicious   warn     JOIN condition doesn't follow the foreign key between the tables
E0031  join-fan-out              suspicious   warn     Aggregate over rows that another one-to-many join multiplies
E1000  parse-error               correctness  error    SQL could not be parsed
```

Each rule has its own page with examples under [Rules](../rules/index.md).

## Levels

A rule is `off`, `warn` or `error`:

- `error`: reported, and makes `sqlsift check` exit with `1`
- `warn`: reported, but doesn't fail the check (unless you set [`--max-warnings`](#ratcheting-warnings))
- `off`: not reported

## Categories

Like [oxlint](https://oxc.rs/docs/guide/usage/linter.html), every rule belongs to a category that sets its default level:

| Category | Default | Meaning |
|----------|---------|---------|
| `correctness` | error | The query fails or does something unintended |
| `suspicious` | warn | The query is most likely wrong |
| `pedantic` | off | Stricter checks that may have false positives |
| `style` | off | Conventions and readability |
| `restriction` | off | Bans on features some codebases don't want |

Current rules are in `correctness` (errors), `suspicious` (warnings) and `restriction` (off), as `sqlsift rules` shows above.

## Changing levels

In `sqlsift.toml`:

```toml
[rules]
E0008 = "warn"             # by code...
ambiguous-column = "off"   # ...or by name

[categories]
suspicious = "error"
```

On the command line, `-A` (allow, i.e. off), `-W` (warn) and `-D` (deny, i.e. error) take a rule code, a rule name or a category, and can be repeated:

```bash
sqlsift check -W ambiguous-column -A E0008 queries/*.sql
```

A rule's own level wins over its category's, and command-line flags win over `sqlsift.toml`. `disable = ["E0006"]` in the file is shorthand for `E0006 = "off"` under `[rules]`.

## Ratcheting warnings

Rolling out a rule on an existing codebase? Make it a warning and cap the number of warnings, so the backlog can only shrink:

```bash
sqlsift check -W missing-required-column --max-warnings 12
```

`--max-warnings <N>` (or `max_warnings = N` in `sqlsift.toml`) makes the check fail when **more than** `N` warnings are reported. `0` fails on any warning without turning warnings into errors.
