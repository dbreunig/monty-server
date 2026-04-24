use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

const TRUNCATE_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FunctionCall {
    pub id: i64,
    pub function_id: String,
    pub status: i64,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
    pub created_at: DateTime<Utc>,
}

impl FunctionCall {
    #[allow(clippy::too_many_arguments)]
    pub async fn insert(
        pool: &SqlitePool,
        function_id: &str,
        status: u16,
        request: Option<&str>,
        response: Option<&str>,
        error: Option<&str>,
        duration_ms: u64,
        truncate: bool,
    ) -> Result<Self> {
        let req = request.map(|s| maybe_truncate(s, truncate));
        let resp = response.map(|s| maybe_truncate(s, truncate));
        let row = sqlx::query_as::<_, FunctionCall>(
            r#"
            INSERT INTO function_calls
                (function_id, status, request_body, response_body, error, duration_ms)
            VALUES (?, ?, ?, ?, ?, ?)
            RETURNING id, function_id, status, request_body, response_body, error, duration_ms, created_at
            "#,
        )
        .bind(function_id)
        .bind(status as i64)
        .bind(req.as_deref())
        .bind(resp.as_deref())
        .bind(error)
        .bind(duration_ms as i64)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_recent(pool: &SqlitePool, function_id: &str, limit: i64) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, FunctionCall>(
            r#"
            SELECT id, function_id, status, request_body, response_body, error, duration_ms, created_at
            FROM function_calls WHERE function_id = ?
            ORDER BY id DESC LIMIT ?
            "#,
        )
        .bind(function_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn prune(pool: &SqlitePool, function_id: &str, keep: i64) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM function_calls
            WHERE function_id = ?
              AND id NOT IN (
                SELECT id FROM function_calls
                WHERE function_id = ?
                ORDER BY id DESC LIMIT ?
              )
            "#,
        )
        .bind(function_id)
        .bind(function_id)
        .bind(keep)
        .execute(pool)
        .await?;
        Ok(())
    }
}

fn maybe_truncate(s: &str, truncate: bool) -> String {
    if !truncate || s.len() <= TRUNCATE_BYTES {
        s.to_string()
    } else {
        let mut end = TRUNCATE_BYTES;
        while !s.is_char_boundary(end) { end -= 1; }
        format!("{}...[truncated]", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncates_when_flag_set() {
        let s = "a".repeat(10_000);
        let t = maybe_truncate(&s, true);
        assert!(t.ends_with("...[truncated]"));
        assert!(t.len() < s.len());
    }

    #[test]
    fn no_truncate_when_flag_clear() {
        let s = "a".repeat(10_000);
        assert_eq!(maybe_truncate(&s, false), s);
    }
}
