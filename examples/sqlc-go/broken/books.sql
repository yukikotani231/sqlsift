-- Mistakes sqlsift catches before `sqlc generate` or the database sees them.
-- This directory is not in sqlc.yaml's query paths.

-- name: ListPublishedBooks :many
SELECT id, titel, published_at
FROM books
WHERE status = 'published';

-- name: ArchiveBook :exec
UPDATE books
SET status = 'retired'
WHERE id = @id;

-- name: CreateBook :one
INSERT INTO books (author_id, title)
VALUES (@author_id, @title)
RETURNING id;
