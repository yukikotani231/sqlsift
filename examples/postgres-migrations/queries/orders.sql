-- Orders of a customer, newest first, with their item count.
SELECT o.id, o.status, o.total, o.placed_at, count(i.sku) AS items
FROM orders o
LEFT JOIN order_items i ON i.order_id = o.id
WHERE o.customer_id = $1
GROUP BY o.id
ORDER BY o.placed_at DESC;

-- Mark an order paid.
UPDATE orders
SET status = 'paid'
WHERE id = $1 AND status = 'pending';

-- Revenue per country over the last 30 days.
WITH recent AS (
    SELECT customer_id, total
    FROM orders
    WHERE status IN ('paid', 'shipped')
      AND placed_at > CURRENT_TIMESTAMP - INTERVAL '30 days'
)
SELECT c.country, sum(r.total) AS revenue, count(*) AS orders
FROM recent r
JOIN customers c ON c.id = r.customer_id
GROUP BY c.country
ORDER BY revenue DESC;
