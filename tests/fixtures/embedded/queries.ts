// Queries in tagged template literals (checked with `embedded_sql_tags`)
import { sql, db, Prisma, prisma } from "./db";

// "Ünïcode" before a query: columns are counted in characters
const greeting = "Ünïcode `not a template`";
const pattern = /`[`'"]/g;

export async function getPost(id: number) {
  return sql`
    SELECT id, title, body FROM posts
    WHERE id = ${id}
  `;
}

export async function listPosts(authorId: number, limit: number) {
  return db.sql`SELECT id, titel FROM posts WHERE author_id = ${authorId} LIMIT ${limit}`;
}

export async function postsByIds(ids: number[]) {
  return sql`SELECT id FROM posts WHERE id IN ${sql(ids)} AND published = ${true}`;
}

export async function fromTable(table: string) {
  return sql`SELECT anything FROM ${sql(table)} WHERE id = ${1}`;
}

export async function authorNames() {
  const filter = `published = ${"true"}`; // an untagged template is not SQL
  return Prisma.sql`SELECT a.name, p.title
    FROM authors a
    JOIN post p ON p.author_id = a.id`;
}

export async function rawCount() {
  return prisma.$queryRaw<{ count: bigint }[]>`SELECT count(*) FROM authors WHERE nme = ${"x"}`;
}
