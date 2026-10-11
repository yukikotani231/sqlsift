# Rules

Every rule has a code, a name and a category. Rules in the `correctness` category report queries the database rejects (or that fail at run time) and are errors by default; rules in the `suspicious` category report valid queries that almost certainly don't do what was meant, and are warnings by default. See [Rules and levels](../guide/rules.md) to change their levels and [Suppressing diagnostics](../guide/suppression.md) for exceptions.

The `E` in a rule code is sqlsift's prefix, not a severity: a rule's level comes from its category and your configuration, so a warning can carry an `E` code too. Codes are assigned in order as rules are added and never change or get reused, so they are safe to keep in config files, suppression comments and baselines. Names are easier to read in config and comments; codes are shorter.

The examples on these pages use this schema:

```sql
CREATE TYPE order_status AS ENUM ('open', 'paid', 'shipped');
CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE,
  created_at TIMESTAMP NOT NULL DEFAULT now()
);
CREATE TABLE orders (
  id SERIAL PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  status order_status NOT NULL DEFAULT 'open',
  total NUMERIC(10, 2) NOT NULL
);
CREATE TABLE coupons (
  code TEXT PRIMARY KEY,
  order_id INTEGER REFERENCES orders(id)
);
```

| Code | Name | Description |
|------|------|-------------|
| [E0001](E0001.md) | `table-not-found` | Referenced table does not exist in schema |
| [E0002](E0002.md) | `column-not-found` | Referenced column does not exist in table |
| [E0003](E0003.md) | `type-mismatch` | Type incompatibility in expression |
| [E0004](E0004.md) | `potential-null-violation` | Potential NOT NULL violation |
| [E0005](E0005.md) | `column-count-mismatch` | Column count doesn't match (INSERT, column aliases, subqueries) |
| [E0006](E0006.md) | `ambiguous-column` | Column reference is ambiguous across tables |
| [E0007](E0007.md) | `join-type-mismatch` | JOIN condition compares incompatible types |
| [E0008](E0008.md) | `missing-required-column` | INSERT omits a NOT NULL column without a default |
| [E0009](E0009.md) | `duplicate-name` | Table alias or CTE name given twice in one query |
| [E0010](E0010.md) | `duplicate-target-column` | Column given twice in an INSERT column list or UPDATE SET |
| [E0011](E0011.md) | `position-out-of-range` | ORDER BY / GROUP BY position is not in the select list |
| [E0012](E0012.md) | `misplaced-aggregate` | Aggregate or window function where it is not allowed |
| [E0013](E0013.md) | `generated-column-write` | INSERT or UPDATE gives a value to a generated column |
| [E0014](E0014.md) | `unmatched-conflict-target` | ON CONFLICT columns match no unique constraint |
| [E0015](E0015.md) | `distinct-order-by` | SELECT DISTINCT ordered by a column it doesn't select |
| [E0016](E0016.md) | `grouping-error` | Column must appear in GROUP BY or be used in an aggregate |
| [E0017](E0017.md) | `value-out-of-range` | Literal too long or out of range for the column type |
| [E0018](E0018.md) | `null-comparison` | Comparison with NULL using = or <> is never true |
| [E0019](E0019.md) | `not-in-with-nulls` | NOT IN over a nullable subquery column |
| [E0020](E0020.md) | `outer-column-in-subquery` | IN subquery selects a column of the outer query |
| [E0021](E0021.md) | `missing-join-condition` | Tables in FROM with no condition linking them |
| [E1000](E1000.md) | `parse-error` | SQL could not be parsed |
