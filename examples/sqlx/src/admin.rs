//! Queries written inline: `query_as!` is checked by sqlx when it compiles,
//! `sqlx::query_scalar` only when it runs. sqlsift checks both against
//! `migrations/` (see the README).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct Overdue {
    pub id: i64,
    pub title: String,
    pub owner: String,
}

pub async fn overdue(pool: &PgPool) -> sqlx::Result<Vec<Overdue>> {
    sqlx::query_as!(
        Overdue,
        r#"
        SELECT t.id, t.title, u.display_name AS "owner!"
        FROM todos t
        JOIN users u ON u.id = t.owner_id
        WHERE t.completed_at IS NULL
          AND t.due_on < CURRENT_DATE
        ORDER BY t.due_on
        "#
    )
    .fetch_all(pool)
    .await
}

pub async fn completed_since(
    pool: &PgPool,
    owner: Uuid,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM todos WHERE owner_id = $1 AND completed_at >= $2",
    )
    .bind(owner)
    .bind(since)
    .fetch_one(pool)
    .await
}
