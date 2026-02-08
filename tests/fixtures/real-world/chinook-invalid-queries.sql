-- Chinook invalid queries (should produce errors)

-- E0001: Table not found
SELECT * FROM songs;

-- E0002: Column not found
SELECT track_id, song_name FROM track;

-- E0002: Column not found in JOIN
SELECT t.name, a.album_name FROM track t JOIN album a ON t.album_id = a.album_id;

-- E0001: Table not found in JOIN
SELECT t.name FROM track t JOIN tags tg ON t.track_id = tg.track_id;

-- E0005: INSERT column count mismatch
INSERT INTO artist (artist_id, name) VALUES (1);

-- E0002: INSERT column not found
INSERT INTO track (track_id, name, song_length) VALUES (1, 'Test', 1000);

-- E0002: UPDATE column not found
UPDATE album SET album_name = 'Test' WHERE album_id = 1;

-- E0002: DELETE WHERE column not found
DELETE FROM invoice WHERE amount > 100;

-- E0001: Table not found in subquery
SELECT name FROM track WHERE genre_id IN (SELECT genre_id FROM music_genres);
