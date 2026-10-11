import { PrismaClient, Prisma } from "@prisma/client";
import { getTopPosts, getUsersWithPostCount } from "@prisma/client/sql";

const prisma = new PrismaClient();

// TypedSQL: the queries live in prisma/sql/*.sql.
export async function topPosts() {
  return prisma.$queryRawTyped(getTopPosts(10));
}

export async function activeAuthors(minPosts: number) {
  return prisma.$queryRawTyped(getUsersWithPostCount(minPosts));
}

// Raw queries in tagged templates are checked too (embedded_sql_tags).
export async function recordView(postId: number) {
  await prisma.$executeRaw`
    UPDATE "Post" SET "viewCount" = "viewCount" + 1 WHERE id = ${postId}
  `;
}

export async function searchPosts(term: string, onlyPublished: boolean) {
  const filter = onlyPublished ? Prisma.sql`AND published` : Prisma.empty;
  return prisma.$queryRaw<{ id: number; title: string }[]>`
    SELECT id, title
    FROM "Post"
    WHERE title ILIKE ${"%" + term + "%"} ${filter}
    ORDER BY "createdAt" DESC
  `;
}
