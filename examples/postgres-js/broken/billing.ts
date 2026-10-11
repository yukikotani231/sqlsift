import { sql } from "../src/db";

export async function upgrade(accountId: string) {
  // accounts has no plan_id column: the plan is stored as text in `plan`.
  await sql`UPDATE accounts SET plan_id = ${"pro"} WHERE id = ${accountId}`;
}

export async function projectsCreatedSince(since: Date) {
  // `id` is in both tables: PostgreSQL rejects it as ambiguous.
  return sql`
    SELECT id, a.email, p.name
    FROM accounts a
    JOIN projects p ON p.account_id = a.id
    WHERE p.created_at >= ${since}
  `;
}

export async function addProject(name: string) {
  // account_id is NOT NULL without a default.
  await sql`INSERT INTO projects (name) VALUES (${name})`;
}
