-- Product listing for a category page.
SELECT p.id, p.sku, p.title, p.price, p.stock
FROM products p
JOIN categories c ON c.id = p.category_id
WHERE c.slug = ? AND p.status = 'active'
ORDER BY p.created_at DESC
LIMIT ? OFFSET ?;

-- Import from the supplier feed: insert or update by SKU.
INSERT INTO products (category_id, sku, title, price, stock)
VALUES (?, ?, ?, ?, ?)
ON DUPLICATE KEY UPDATE
    title = VALUES(title),
    price = VALUES(price),
    stock = VALUES(stock);

-- Hide products that have been out of stock for a while.
UPDATE products
SET status = 'hidden'
WHERE stock = 0 AND updated_at < NOW() - INTERVAL 90 DAY;
