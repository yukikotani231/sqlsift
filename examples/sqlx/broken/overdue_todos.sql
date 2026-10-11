-- Written before migration 20260320110000 replaced todos.done with
-- completed_at. With SQLX_OFFLINE and a stale .sqlx cache it still compiles;
-- it fails once the cache is prepared against the migrated database.
SELECT id, title, due_on
FROM todos
WHERE owner_id = $1
  AND NOT done
  AND due_on < CURRENT_DATE
ORDER BY due_on
