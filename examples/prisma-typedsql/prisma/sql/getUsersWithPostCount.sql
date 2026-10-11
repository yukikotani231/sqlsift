-- @param {Int} $1:minPosts
SELECT u.id, u.name, u.email, COUNT(p.id)::int AS "postCount"
FROM "User" u
LEFT JOIN "Post" p ON p."authorId" = u.id AND p.published
GROUP BY u.id, u.name, u.email
HAVING COUNT(p.id) >= $1
ORDER BY "postCount" DESC
