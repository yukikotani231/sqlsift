-- 'archived' is not one of the status ENUM's values.
UPDATE products
SET status = 'archived'
WHERE id = ?;

-- The reviews table has no user_id: the author's name is in `author`.
SELECT r.user_id, r.rating, r.body
FROM reviews r
WHERE r.product_id = ?;

-- Joined on the wrong column: sku is a string and product_id a number, which
-- MySQL silently converts, so the join matches nothing.
SELECT r.id, p.title
FROM reviews r
JOIN products p ON p.sku = r.product_id;
