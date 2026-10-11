import { sql } from "./db";

export async function recordEvents(rows: { project_id: number; kind: string }[]) {
  await sql`INSERT INTO events ${sql(rows, "project_id", "kind")}`;
}

export async function dailyActivity(accountId: string, days: number) {
  return sql`
    SELECT p.name, date_trunc('day', e.occurred_at) AS day, count(*) AS events
    FROM events e
    JOIN projects p ON p.id = e.project_id
    WHERE p.account_id = ${accountId}
      AND e.occurred_at > now() - make_interval(days => ${days})
    GROUP BY p.name, day
    ORDER BY day DESC, events DESC
  `;
}

export async function accountsOnPlan(plans: string[]) {
  return sql`
    SELECT a.id, a.email, count(p.id) AS projects
    FROM accounts a
    LEFT JOIN projects p ON p.account_id = a.id AND p.archived_at IS NULL
    WHERE a.plan IN ${sql(plans)}
    GROUP BY a.id, a.email
  `;
}
