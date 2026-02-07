-- Valid queries against the Sakila schema

SELECT actor_id, first_name, last_name FROM actor;

SELECT film_id, title, description, release_year, rental_rate FROM film;

SELECT f.title, c.name AS category
FROM film f
JOIN film_category fc ON f.film_id = fc.film_id
JOIN category c ON fc.category_id = c.category_id;

SELECT customer_id, first_name, last_name, email FROM customer WHERE active = 1;

SELECT r.rental_id, c.first_name, c.last_name, i.film_id
FROM rental r
JOIN customer c ON r.customer_id = c.customer_id
JOIN inventory i ON r.inventory_id = i.inventory_id;

INSERT INTO category (category_id, name, last_update) VALUES (99, 'Test', CURRENT_TIMESTAMP);
