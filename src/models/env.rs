use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Env {
    pub id: String,
    pub user_id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Env {
    pub fn new_id() -> String {
        Uuid::new_v4().simple().to_string()
    }

    pub async fn create(pool: &SqlitePool, user_id: i64, name: &str) -> Result<Self> {
        let id = Self::new_id();
        let row = sqlx::query_as::<_, Env>(
            r#"
            INSERT INTO envs (id, user_id, name)
            VALUES (?, ?, ?)
            RETURNING id, user_id, name, created_at, updated_at
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(name)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Env>(
            r#"
            SELECT id, user_id, name, created_at, updated_at
            FROM envs WHERE user_id = ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn find_for_user(
        pool: &SqlitePool,
        user_id: i64,
        id: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Env>(
            r#"
            SELECT id, user_id, name, created_at, updated_at
            FROM envs WHERE user_id = ? AND id = ?
            "#,
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn delete_for_user(pool: &SqlitePool, user_id: i64, id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM envs WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn touch(pool: &SqlitePool, id: &str) -> Result<()> {
        sqlx::query("UPDATE envs SET updated_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
