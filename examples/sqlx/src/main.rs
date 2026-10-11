//! A todo service whose SQL lives in `queries/*.sql`, loaded at compile time
//! with `sqlx::query_file!` / `sqlx::query_file_as!`. sqlsift checks those
//! files against `migrations/` without a database (see the README).

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::Type)]
#[sqlx(type_name = "todo_priority", rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

#[derive(Debug)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct OpenTodo {
    pub id: i64,
    pub title: String,
    pub priority: Priority,
    pub due_on: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_user(pool: &PgPool, email: &str, name: &str) -> sqlx::Result<User> {
    sqlx::query_file_as!(User, "queries/create_user.sql", email, name)
        .fetch_one(pool)
        .await
}

pub async fn user_by_email(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_file_as!(User, "queries/user_by_email.sql", email)
        .fetch_optional(pool)
        .await
}

pub async fn create_todo(
    pool: &PgPool,
    owner: Uuid,
    title: &str,
    priority: Priority,
    due_on: Option<NaiveDate>,
) -> sqlx::Result<i64> {
    let row = sqlx::query_file!(
        "queries/create_todo.sql",
        owner,
        title,
        priority as Priority,
        due_on
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

pub async fn open_todos(pool: &PgPool, owner: Uuid) -> sqlx::Result<Vec<OpenTodo>> {
    sqlx::query_file_as!(OpenTodo, "queries/open_todos.sql", owner)
        .fetch_all(pool)
        .await
}

pub async fn complete_todo(pool: &PgPool, id: i64, owner: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query_file!("queries/complete_todo.sql", id, owner)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn print_stats(pool: &PgPool, user: Uuid) -> sqlx::Result<()> {
    let stats = sqlx::query_file!("queries/todo_stats.sql", user)
        .fetch_one(pool)
        .await?;
    println!(
        "{}: {} todos, {} completed, {} overdue",
        stats.display_name, stats.total, stats.completed, stats.overdue
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    sqlx::migrate!().run(&pool).await?;

    let user = match user_by_email(&pool, "ada@example.com").await? {
        Some(user) => user,
        None => create_user(&pool, "ada@example.com", "Ada").await?,
    };
    let id = create_todo(&pool, user.id, "Write the report", Priority::High, None).await?;
    for todo in open_todos(&pool, user.id).await? {
        println!("{todo:?}");
    }
    complete_todo(&pool, id, user.id).await?;
    print_stats(&pool, user.id).await?;
    Ok(())
}
