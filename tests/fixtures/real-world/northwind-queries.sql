-- Northwind schema queries
-- Covers: SELECT, JOIN, INSERT, UPDATE, DELETE, subqueries, CTEs

-- Basic SELECT
SELECT order_id, customer_id, employee_id, order_date, freight FROM orders;
SELECT product_id, product_name, unit_price, units_in_stock FROM products;
SELECT employee_id, first_name, last_name, title, hire_date FROM employees;
SELECT category_id, category_name, description FROM categories;

-- Multi-table JOINs
SELECT o.order_id, o.order_date, c.company_name, c.contact_name
FROM orders o
JOIN customers c ON o.customer_id = c.customer_id;

SELECT od.order_id, p.product_name, od.unit_price, od.quantity, od.discount
FROM order_details od
JOIN products p ON od.product_id = p.product_id;

SELECT e.first_name, e.last_name, t.territory_description
FROM employees e
JOIN employee_territories et ON e.employee_id = et.employee_id
JOIN territories t ON et.territory_id = t.territory_id;

SELECT s.company_name, p.product_name, c.category_name
FROM suppliers s
JOIN products p ON s.supplier_id = p.supplier_id
JOIN categories c ON p.category_id = c.category_id;

SELECT o.order_id, o.order_date, e.first_name, e.last_name, sh.company_name
FROM orders o
JOIN employees e ON o.employee_id = e.employee_id
JOIN shippers sh ON o.ship_via = sh.shipper_id;

-- INSERT
INSERT INTO region (region_id, region_description) VALUES (99, 'Test Region');
INSERT INTO shippers (shipper_id, company_name, phone) VALUES (99, 'Test Shipper', '555-0000');

-- UPDATE
UPDATE products SET unit_price = 25.00 WHERE product_id = 1;
UPDATE customers SET contact_name = 'Updated Name' WHERE customer_id = 'ALFKI';
UPDATE employees SET title = 'Senior Rep' WHERE employee_id = 1;

-- DELETE
DELETE FROM order_details WHERE order_id = 10248;
DELETE FROM orders WHERE order_id = 10248;

-- Subqueries
SELECT product_name, unit_price FROM products
WHERE unit_price > (SELECT unit_price FROM products WHERE product_name = 'Chai');

SELECT company_name FROM customers
WHERE customer_id IN (
    SELECT o.customer_id FROM orders o WHERE o.order_date > '1997-01-01'
);

SELECT p.product_name FROM products p
WHERE EXISTS (
    SELECT 1 FROM order_details od WHERE od.product_id = p.product_id AND od.quantity > 50
);

-- CTEs
WITH high_value_orders AS (
    SELECT order_id, customer_id, freight
    FROM orders
    WHERE freight > 100
)
SELECT hvo.order_id, c.company_name, hvo.freight
FROM high_value_orders hvo
JOIN customers c ON hvo.customer_id = c.customer_id;

WITH product_sales AS (
    SELECT p.product_id, p.product_name, od.quantity
    FROM products p
    JOIN order_details od ON p.product_id = od.product_id
)
SELECT product_name, SUM(quantity) AS total_qty
FROM product_sales
GROUP BY product_name;
