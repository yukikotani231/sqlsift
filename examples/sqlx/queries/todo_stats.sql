SELECT u.id,
       u.display_name,
       count(t.id) AS "total!",
       count(t.id) FILTER (WHERE t.completed_at IS NOT NULL) AS "completed!",
       count(t.id) FILTER (WHERE t.completed_at IS NULL AND t.due_on < CURRENT_DATE) AS "overdue!"
FROM users u
LEFT JOIN todos t ON t.owner_id = u.id
WHERE u.id = $1
GROUP BY u.id, u.display_name
