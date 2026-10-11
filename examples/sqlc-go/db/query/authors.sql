-- name: GetAuthor :one
SELECT id, name, email, bio, created_at
FROM authors
WHERE id = $1;

-- name: ListAuthors :many
SELECT id, name, email
FROM authors
ORDER BY name
LIMIT sqlc.arg(page_size) OFFSET sqlc.arg(page_offset);

-- name: CreateAuthor :one
INSERT INTO authors (name, email, bio)
VALUES (@name, @email, sqlc.narg(bio))
RETURNING id, created_at;

-- name: UpdateAuthorBio :exec
UPDATE authors
SET bio = sqlc.narg(bio)
WHERE id = @id;

-- name: DeleteAuthor :exec
DELETE FROM authors
WHERE id = $1;
