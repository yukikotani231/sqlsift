-- name: ListBooksByAuthor :many
SELECT b.id, b.title, b.status, b.price_cents, b.published_at
FROM books b
WHERE b.author_id = @author_id
  AND b.status <> 'archived'
ORDER BY b.published_at DESC NULLS LAST;

-- name: GetBooksByIDs :many
SELECT id, title, isbn
FROM books
WHERE id = ANY(@ids::bigint[]);

-- name: PublishBook :one
UPDATE books
SET status = 'published', published_at = now()
WHERE id = @id AND status = 'draft'
RETURNING id, published_at;

-- name: AuthorStats :many
SELECT a.id, a.name, count(b.id) AS book_count, coalesce(sum(b.price_cents), 0) AS total_price_cents
FROM authors a
LEFT JOIN books b ON b.author_id = a.id AND b.status = 'published'
GROUP BY a.id, a.name
HAVING count(b.id) >= sqlc.arg(min_books)
ORDER BY book_count DESC;

-- name: SearchBooksByTag :many
SELECT id, title, tags
FROM books
WHERE tags @> ARRAY[@tag::text]
ORDER BY title;
