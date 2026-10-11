-- @param {Int} $1:authorId
-- The Post model has no updatedAt field, so there is no "updatedAt" column.
SELECT id, title, "updatedAt"
FROM "Post"
WHERE "authorId" = $1 AND NOT published
ORDER BY "updatedAt" DESC
