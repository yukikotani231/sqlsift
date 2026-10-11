-- @param {Int} $1:limit
SELECT p.id, p.title, p."viewCount", u.name AS "authorName"
FROM "Post" p
JOIN "User" u ON u.id = p."authorId"
WHERE p.published = true
  AND p."createdAt" > now() - interval '30 days'
ORDER BY p."viewCount" DESC
LIMIT $1
