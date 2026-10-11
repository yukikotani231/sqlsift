// Written before the last migration replaced todos.done with completed_at.
// It still compiles: sqlx::query_scalar is only checked when it runs.

use sqlx::PgPool;
use uuid::Uuid;

pub async fn open_count(pool: &PgPool, owner: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM todos WHERE owner_id = $1 AND NOT done")
        .bind(owner)
        .fetch_one(pool)
        .await
}
