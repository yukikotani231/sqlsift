import { PrismaClient } from "@prisma/client";

const prisma = new PrismaClient();

export async function promote(userId: number) {
  // 'OWNER' is not a value of the "Role" enum.
  await prisma.$executeRaw`
    UPDATE "User" SET role = 'OWNER' WHERE id = ${userId}
  `;
}

export async function signupsPerDay() {
  // Prisma names the table after the model: "User", not "Users".
  return prisma.$queryRaw`
    SELECT date_trunc('day', "createdAt") AS day, count(*) AS signups
    FROM "Users"
    GROUP BY 1
    ORDER BY 1
  `;
}
