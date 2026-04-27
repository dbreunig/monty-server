use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ReplCall {
    pub id: i64,
    pub env_id: String,
    pub code: String,
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ReplCall {
    pub async fn create(
        pool: &SqlitePool,
        env_id: &str,
        code: &str,
        stdout: &str,
        stderr: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<Self> {
        let row = sqlx::query_as::<_, ReplCall>(
            r#"
            INSERT INTO repl_calls (env_id, code, stdout, stderr, result, error)
            VALUES (?, ?, ?, ?, ?, ?)
            RETURNING id, env_id, code, stdout, stderr, result, error, created_at
            "#,
        )
        .bind(env_id)
        .bind(code)
        .bind(stdout)
        .bind(stderr)
        .bind(result)
        .bind(error)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_for_env(pool: &SqlitePool, env_id: &str) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, ReplCall>(
            r#"
            SELECT id, env_id, code, stdout, stderr, result, error, created_at
            FROM repl_calls WHERE env_id = ?
            ORDER BY id ASC
            "#,
        )
        .bind(env_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }
}
