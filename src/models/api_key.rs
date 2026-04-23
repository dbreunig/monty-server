use anyhow::{Result, anyhow};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiKey {
    pub async fn create(pool: &SqlitePool, user_id: i64, name: &str) -> Result<(Self, String)> {
        let mut bytes = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut bytes);
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        let plaintext = format!("mk_{encoded}");
        let prefix = plaintext[..11].to_string();

        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(plaintext.as_bytes(), &salt)
            .map_err(|e| anyhow!("argon2 hash failed: {e}"))?
            .to_string();

        let row = sqlx::query_as::<_, ApiKey>(
            r#"
            INSERT INTO api_keys (user_id, name, token_hash, prefix)
            VALUES (?, ?, ?, ?)
            RETURNING id, user_id, name, prefix, created_at, last_used_at
            "#,
        )
        .bind(user_id)
        .bind(name)
        .bind(&hash)
        .bind(&prefix)
        .fetch_one(pool)
        .await?;

        Ok((row, plaintext))
    }

    pub async fn verify(pool: &SqlitePool, plaintext: &str) -> Result<Option<Self>> {
        if plaintext.len() < 11 || !plaintext.starts_with("mk_") {
            return Ok(None);
        }
        let prefix = &plaintext[..11];

        let candidates = sqlx::query_as::<_, (i64, i64, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>)>(
            r#"
            SELECT id, user_id, name, prefix, token_hash, created_at, last_used_at
            FROM api_keys WHERE prefix = ?
            "#,
        )
        .bind(prefix)
        .fetch_all(pool)
        .await?;

        for (id, user_id, name, prefix_row, token_hash, created_at, last_used_at) in candidates {
            let parsed = match PasswordHash::new(&token_hash) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if Argon2::default()
                .verify_password(plaintext.as_bytes(), &parsed)
                .is_ok()
            {
                sqlx::query("UPDATE api_keys SET last_used_at = datetime('now') WHERE id = ?")
                    .bind(id)
                    .execute(pool)
                    .await?;
                return Ok(Some(ApiKey {
                    id,
                    user_id,
                    name,
                    prefix: prefix_row,
                    created_at,
                    last_used_at,
                }));
            }
        }
        Ok(None)
    }

    pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, ApiKey>(
            r#"
            SELECT id, user_id, name, prefix, created_at, last_used_at
            FROM api_keys WHERE user_id = ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete(pool: &SqlitePool, user_id: i64, id: i64) -> Result<bool> {
        let res = sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
