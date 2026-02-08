-- Chinook schema queries
-- Covers: SELECT, JOIN, INSERT, UPDATE, DELETE, subqueries, CTEs

-- Basic SELECT
SELECT album_id, title, artist_id FROM album;
SELECT artist_id, name FROM artist;
SELECT track_id, name, composer, milliseconds, unit_price FROM track;
SELECT customer_id, first_name, last_name, email FROM customer;
SELECT invoice_id, customer_id, invoice_date, total FROM invoice;

-- Multi-table JOINs
SELECT t.name AS track, a.title AS album, ar.name AS artist
FROM track t
JOIN album a ON t.album_id = a.album_id
JOIN artist ar ON a.artist_id = ar.artist_id;

SELECT c.first_name, c.last_name, i.invoice_date, i.total
FROM customer c
JOIN invoice i ON c.customer_id = i.customer_id;

SELECT il.invoice_line_id, t.name AS track, il.unit_price, il.quantity
FROM invoice_line il
JOIN track t ON il.track_id = t.track_id;

SELECT p.name AS playlist, t.name AS track
FROM playlist p
JOIN playlist_track pt ON p.playlist_id = pt.playlist_id
JOIN track t ON pt.track_id = t.track_id;

SELECT t.name AS track, g.name AS genre, mt.name AS media_type
FROM track t
JOIN genre g ON t.genre_id = g.genre_id
JOIN media_type mt ON t.media_type_id = mt.media_type_id;

SELECT c.first_name, c.last_name, e.first_name AS rep_first, e.last_name AS rep_last
FROM customer c
JOIN employee e ON c.support_rep_id = e.employee_id;

-- INSERT
INSERT INTO genre (genre_id, name) VALUES (99, 'J-Pop');
INSERT INTO artist (artist_id, name) VALUES (999, 'Test Artist');
INSERT INTO playlist (playlist_id, name) VALUES (99, 'My Playlist');

-- UPDATE
UPDATE track SET unit_price = 1.29 WHERE track_id = 1;
UPDATE customer SET email = 'new@example.com' WHERE customer_id = 1;
UPDATE album SET title = 'Updated Title' WHERE album_id = 1;

-- DELETE
DELETE FROM playlist_track WHERE playlist_id = 1;
DELETE FROM invoice_line WHERE invoice_line_id = 1;

-- Subqueries
SELECT name FROM track
WHERE album_id IN (SELECT a.album_id FROM album a WHERE a.artist_id = 1);

SELECT first_name, last_name FROM customer
WHERE customer_id IN (
    SELECT i.customer_id FROM invoice i WHERE i.total > 20
);

SELECT a.title FROM album a
WHERE EXISTS (
    SELECT 1 FROM track t WHERE t.album_id = a.album_id AND t.milliseconds > 300000
);

-- Scalar subquery
SELECT ar.name,
    (SELECT COUNT(album_id) FROM album a WHERE a.artist_id = ar.artist_id) AS album_count
FROM artist ar;

-- CTEs
WITH long_tracks AS (
    SELECT track_id, name, milliseconds, album_id
    FROM track
    WHERE milliseconds > 300000
)
SELECT lt.name AS track, a.title AS album
FROM long_tracks lt
JOIN album a ON lt.album_id = a.album_id;

WITH customer_totals AS (
    SELECT c.customer_id, c.first_name, c.last_name
    FROM customer c
),
invoice_summary AS (
    SELECT customer_id, SUM(total) AS total_spent
    FROM invoice
    GROUP BY customer_id
)
SELECT ct.first_name, ct.last_name, ins.total_spent
FROM customer_totals ct
JOIN invoice_summary ins ON ct.customer_id = ins.customer_id;
