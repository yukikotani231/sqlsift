-- Pagila schema queries
-- Covers: SELECT, JOIN, INSERT, UPDATE, DELETE, subqueries, CTEs

-- Basic SELECT
SELECT actor_id, first_name, last_name FROM actor;
SELECT film_id, title, description, release_year, rental_rate FROM film;
SELECT customer_id, first_name, last_name, email, active FROM customer;

-- Multi-table JOINs
SELECT f.title, c.name AS category
FROM film f
JOIN film_category fc ON f.film_id = fc.film_id
JOIN category c ON fc.category_id = c.category_id;

SELECT c.first_name, c.last_name, c.email
FROM customer c
JOIN address a ON c.address_id = a.address_id
JOIN city ci ON a.city_id = ci.city_id
JOIN country co ON ci.country_id = co.country_id;

SELECT r.rental_id, r.rental_date, c.first_name, c.last_name, i.film_id
FROM rental r
JOIN customer c ON r.customer_id = c.customer_id
JOIN inventory i ON r.inventory_id = i.inventory_id;

SELECT s.first_name, s.last_name, st.store_id
FROM staff s
JOIN store st ON s.staff_id = st.manager_staff_id;

SELECT p.payment_id, p.amount, p.payment_date, c.first_name
FROM payment p
JOIN customer c ON p.customer_id = c.customer_id;

-- INSERT
INSERT INTO language (language_id, name, last_update) VALUES (99, 'Japanese', CURRENT_TIMESTAMP);
INSERT INTO category (category_id, name, last_update) VALUES (100, 'Anime', CURRENT_TIMESTAMP);

-- UPDATE
UPDATE customer SET active = 0 WHERE customer_id = 1;
UPDATE film SET rental_rate = 5.99 WHERE film_id = 1;
UPDATE address SET phone = '000-000-0000' WHERE address_id = 1;

-- DELETE
DELETE FROM payment WHERE payment_id = 1;
DELETE FROM rental WHERE rental_id = 1;

-- Subqueries
SELECT first_name, last_name FROM customer
WHERE customer_id IN (SELECT r.customer_id FROM rental r WHERE r.rental_date > '2020-01-01');

SELECT title FROM film
WHERE film_id IN (
    SELECT i.film_id FROM inventory i
    WHERE i.store_id = 1
);

SELECT f.title, f.rental_rate FROM film f
WHERE EXISTS (
    SELECT 1 FROM film_actor fa
    JOIN actor a ON fa.actor_id = a.actor_id
    WHERE fa.film_id = f.film_id AND a.last_name = 'SMITH'
);

-- Scalar subquery
SELECT first_name, last_name,
    (SELECT COUNT(rental_id) FROM rental r WHERE r.customer_id = c.customer_id) AS rental_count
FROM customer c;

-- CTEs
WITH active_customers AS (
    SELECT customer_id, first_name, last_name FROM customer WHERE active = 1
)
SELECT ac.first_name, ac.last_name, r.rental_date
FROM active_customers ac
JOIN rental r ON ac.customer_id = r.customer_id;

WITH film_stats AS (
    SELECT f.film_id, f.title, f.rental_rate
    FROM film f
    WHERE f.rental_rate > 3.00
),
rental_counts AS (
    SELECT i.film_id, COUNT(r.rental_id) AS cnt
    FROM inventory i
    JOIN rental r ON i.inventory_id = r.inventory_id
    GROUP BY i.film_id
)
SELECT fs.title, fs.rental_rate, rc.cnt
FROM film_stats fs
JOIN rental_counts rc ON fs.film_id = rc.film_id;
