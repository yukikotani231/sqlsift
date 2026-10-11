-- Average rating per product, verified reviews only.
SELECT p.id, p.title, AVG(r.rating) AS avg_rating, COUNT(*) AS review_count
FROM products p
JOIN reviews r ON r.product_id = p.id
WHERE r.is_verified = 1
GROUP BY p.id, p.title
HAVING review_count >= ?
ORDER BY avg_rating DESC;

-- Post a review.
INSERT INTO reviews (product_id, author, rating, body)
VALUES (?, ?, ?, ?);
