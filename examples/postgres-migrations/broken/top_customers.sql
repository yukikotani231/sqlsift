-- A report query nobody updated when migration 0003 renamed
-- customers.name to full_name. It still parses, and it fails only when it
-- runs against the migrated database.
SELECT c.id, c.name, sum(o.total) AS lifetime_value
FROM customers c
JOIN orders o ON o.customer_id = c.id
WHERE o.status <> 'cancelled'
GROUP BY c.id, c.name
ORDER BY lifetime_value DESC
LIMIT 20;
