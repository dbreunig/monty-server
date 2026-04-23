use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub github_id: i64,
    pub github_login: String,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub async fn upsert(
        pool: &SqlitePool,
        github_id: i64,
        github_login: &str,
        email: Option<&str>,
    ) -> Result<Self> {
        let row = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (github_id, github_login, email)
            VALUES (?, ?, ?)
            ON CONFLICT(github_id) DO UPDATE SET
                github_login = excluded.github_login,
                email = excluded.email
            RETURNING id, github_id, github_login, email, created_at
            "#,
        )
        .bind(github_id)
        .bind(github_login)
        .bind(email)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, User>(
            r#"
            SELECT id, github_id, github_login, email, created_at
            FROM users WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }
}
