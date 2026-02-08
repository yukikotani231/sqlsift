-- Northwind invalid queries (should produce errors)

-- E0001: Table not found
SELECT * FROM invoices;

-- E0002: Column not found
SELECT order_id, total_amount FROM orders;

-- E0001: Table not found in JOIN
SELECT o.order_id FROM orders o JOIN line_items li ON o.order_id = li.order_id;

-- E0005: INSERT column count mismatch
INSERT INTO region (region_id, region_description) VALUES (1);

-- E0002: UPDATE column not found
UPDATE products SET price = 10.00 WHERE product_id = 1;

-- E0002: DELETE WHERE column not found
DELETE FROM orders WHERE total > 100;
