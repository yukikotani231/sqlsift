-- sqlx type overrides live in the column aliases: "priority: Priority" maps
-- the enum to a Rust type, "id!" forces non-null. To sqlsift they are
-- ordinary quoted aliases.
SELECT id AS "id!",
       title,
       priority AS "priority: Priority",
       due_on,
       created_at
FROM todos
WHERE owner_id = $1
  AND completed_at IS NULL
ORDER BY priority DESC, due_on NULLS LAST, created_at
