import { sql } from "./db";

export interface Project {
  id: number;
  name: string;
  created_at: Date;
}

export async function listProjects(accountId: string, includeArchived = false) {
  return sql<Project[]>`
    SELECT id, name, created_at
    FROM projects
    WHERE account_id = ${accountId}
    ${includeArchived ? sql`` : sql`AND archived_at IS NULL`}
    ORDER BY created_at DESC
  `;
}

export async function createProject(accountId: string, name: string) {
  const [project] = await sql<Project[]>`
    INSERT INTO projects (account_id, name)
    VALUES (${accountId}, ${name})
    RETURNING id, name, created_at
  `;
  return project;
}

export async function renameProject(id: number, patch: { name: string }) {
  await sql`UPDATE projects SET ${sql(patch, "name")} WHERE id = ${id}`;
}

export async function archiveProject(id: number) {
  await sql`UPDATE projects SET archived_at = now() WHERE id = ${id}`;
}
