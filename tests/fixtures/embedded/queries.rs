//! Queries written inline with sqlx (checked by sqlsift without a database)

use sqlx::PgPool;

pub struct Post {
    pub id: i32,
    pub title: String,
}

pub async fn posts_by(pool: &PgPool, author_id: i32) -> sqlx::Result<Vec<Post>> {
    sqlx::query_as!(
        Post,
        r#"SELECT id, titel AS "title" FROM posts WHERE author_id = $1"#,
        author_id
    )
    .fetch_all(pool)
    .await
}

pub async fn publish<'a>(pool: &'a PgPool, id: i32) -> sqlx::Result<()> {
    sqlx::query!("UPDATE posts SET published = true WHERE id = $1", id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn author_count(pool: &PgPool) -> sqlx::Result<i64> {
    // Checked at run time only, but sqlsift checks it too
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM author")
        .fetch_one(pool)
        .await
}

pub async fn rename(pool: &PgPool, id: i32, name: &str) -> sqlx::Result<()> {
    // sqlsift:disable E0002
    sqlx::query!("UPDATE authors SET nme = $2 WHERE id = $1", id, name)
        .execute(pool)
        .await?;
    let label = format!("SELECT {} FROM nowhere", name);
    println!("{label}");
    Ok(())
}
