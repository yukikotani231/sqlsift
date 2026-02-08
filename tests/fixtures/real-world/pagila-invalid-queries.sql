-- Pagila invalid queries (should produce errors)

-- E0001: Table not found
SELECT * FROM nonexistent_table;

-- E0002: Column not found
SELECT nonexistent_column FROM actor;

-- E0002: Column not found in JOIN
SELECT f.title, f.nonexistent FROM film f;

-- E0001: Table not found in JOIN
SELECT a.first_name FROM actor a JOIN fake_table ft ON a.actor_id = ft.actor_id;

-- E0005: INSERT column count mismatch
INSERT INTO actor (actor_id, first_name, last_name) VALUES (1, 'John');

-- E0002: INSERT column not found
INSERT INTO actor (actor_id, first_name, fake_col) VALUES (1, 'John', 'Doe');

-- E0002: UPDATE column not found
UPDATE film SET fake_column = 'test' WHERE film_id = 1;

-- E0002: DELETE WHERE column not found
DELETE FROM customer WHERE fake_column = 1;

-- E0002: Column not found in subquery
SELECT first_name FROM customer WHERE customer_id IN (SELECT fake_id FROM rental);

-- E0001: Table not found in CTE body
WITH bad_cte AS (SELECT id FROM nonexistent)
SELECT id FROM bad_cte;
