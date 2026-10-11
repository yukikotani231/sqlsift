UPDATE todos
SET completed_at = now()
WHERE id = $1 AND owner_id = $2 AND completed_at IS NULL
