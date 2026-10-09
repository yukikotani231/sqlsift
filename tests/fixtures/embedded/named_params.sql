-- sqlc query file with named parameters: `@name`, sqlc.arg(), sqlc.narg(), sqlc.slice()

-- name: ListPostsAfter :many
SELECT id, title FROM posts
WHERE id > @after_id AND author_id = sqlc.arg(author_id)
ORDER BY id
LIMIT @page_size;

-- name: SearchPosts :many
SELECT id, title FROM posts
WHERE title = sqlc.narg('title')::text OR sqlc.narg('title') IS NULL;

-- name: GetPostsByIDs :many
SELECT id, title FROM posts WHERE id IN (sqlc.slice(ids));

-- name: UpdatePostTitle :exec
UPDATE posts SET title = @title WHERE id = @id;

-- name: ListProlificAuthors :many
SELECT author_id, count(*) FROM posts
GROUP BY author_id
HAVING count(*) > sqlc.arg(min_count);

-- name: GetDraft :one
SELECT id, titel FROM posts WHERE id = @id AND NOT published;
