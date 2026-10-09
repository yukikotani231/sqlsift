-- sqlc query file: each query is named by a `-- name:` comment

-- name: GetPost :one
SELECT id, title, body FROM posts
WHERE id = $1;

-- name: ListPosts :many
SELECT id, titel FROM posts
WHERE author_id = $1
ORDER BY id;

-- name: CreatePost :one
INSERT INTO posts (author_id, title, body)
VALUES ($1, $2, $3)
RETURNING id;

-- name: GetAuthor :one
SELECT id, name FROM author
WHERE id = $1;
