# Rules

Every rule has a code, a name and a category. All current rules are in the `correctness` category and are errors by default. See [Rules and levels](../guide/rules.md) to change their levels and [Suppressing diagnostics](../guide/suppression.md) for exceptions.

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
```

| Code | Name | Description |
|------|------|-------------|
| [E0001](E0001.md) | `table-not-found` | Referenced table does not exist in schema |
| [E0002](E0002.md) | `column-not-found` | Referenced column does not exist in table |
| [E0003](E0003.md) | `type-mismatch` | Type incompatibility in expression |
| [E0004](E0004.md) | `potential-null-violation` | Potential NOT NULL violation |
| [E0005](E0005.md) | `column-count-mismatch` | INSERT column count doesn't match values |
| [E0006](E0006.md) | `ambiguous-column` | Column reference is ambiguous across tables |
| [E0007](E0007.md) | `join-type-mismatch` | JOIN condition compares incompatible types |
| [E0008](E0008.md) | `missing-required-column` | INSERT omits a NOT NULL column without a default |
| [E1000](E1000.md) | `parse-error` | SQL could not be parsed |
