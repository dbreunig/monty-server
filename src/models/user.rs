use anyhow::{Result, anyhow};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub github_id: Option<i64>,
    pub github_login: Option<String>,
    pub email: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub is_admin: bool,
    pub approved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl User {
    pub fn is_approved(&self) -> bool {
        self.approved_at.is_some()
    }

    pub fn display_name(&self) -> &str {
        self.github_login
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or("(unknown)")
    }

    /// Upsert a user identified by GitHub id. Never flips `is_admin` or
    /// touches `approved_at` of an existing row.
    pub async fn upsert_github(
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
            RETURNING id, github_id, github_login, email, password_hash,
                      is_admin, approved_at, created_at
            "#,
        )
        .bind(github_id)
        .bind(github_login)
        .bind(email)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    /// Creates a new local (email + password) user in pending-approval state.
    /// Fails if the email is already used by another local account.
    pub async fn create_local(pool: &SqlitePool, email: &str, password: &str) -> Result<Self> {
        let hash = hash_password(password)?;
        let row = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (email, password_hash)
            VALUES (?, ?)
            RETURNING id, github_id, github_login, email, password_hash,
                      is_admin, approved_at, created_at
            "#,
        )
        .bind(email)
        .bind(&hash)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    /// Seeds or updates the single admin user identified by `email`. If the
    /// admin exists, the password is rotated to the one supplied — this lets
    /// operators reset it by changing the env var. The row is always
    /// auto-approved and marked `is_admin = 1`.
    pub async fn ensure_admin(pool: &SqlitePool, email: &str, password: &str) -> Result<Self> {
        let hash = hash_password(password)?;
        let row = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (email, password_hash, is_admin, approved_at)
            VALUES (?, ?, 1, datetime('now'))
            ON CONFLICT(email) WHERE password_hash IS NOT NULL DO UPDATE SET
                password_hash = excluded.password_hash,
                is_admin = 1,
                approved_at = COALESCE(approved_at, datetime('now'))
            RETURNING id, github_id, github_login, email, password_hash,
                      is_admin, approved_at, created_at
            "#,
        )
        .bind(email)
        .bind(&hash)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_local_by_email(pool: &SqlitePool, email: &str) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, User>(
            r#"
            SELECT id, github_id, github_login, email, password_hash,
                   is_admin, approved_at, created_at
            FROM users WHERE email = ? AND password_hash IS NOT NULL
            "#,
        )
        .bind(email)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    /// Verifies a local-login password. Returns the user on success, None on
    /// missing/mismatch (constant-ish time regardless of which).
    pub async fn verify_local_login(
        pool: &SqlitePool,
        email: &str,
        password: &str,
    ) -> Result<Option<Self>> {
        let Some(user) = Self::find_local_by_email(pool, email).await? else {
            // Run a dummy argon2 compare to equalise timing.
            let _ = verify_password("dummy", DUMMY_HASH);
            return Ok(None);
        };
        let Some(hash) = user.password_hash.as_deref() else {
            return Ok(None);
        };
        if verify_password(password, hash) {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, User>(
            r#"
            SELECT id, github_id, github_login, email, password_hash,
                   is_admin, approved_at, created_at
            FROM users WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_by_github_id(pool: &SqlitePool, github_id: i64) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, User>(
            r#"
            SELECT id, github_id, github_login, email, password_hash,
                   is_admin, approved_at, created_at
            FROM users WHERE github_id = ?
            "#,
        )
        .bind(github_id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_pending(pool: &SqlitePool) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, User>(
            r#"
            SELECT id, github_id, github_login, email, password_hash,
                   is_admin, approved_at, created_at
            FROM users WHERE approved_at IS NULL
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn approve(pool: &SqlitePool, id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE users SET approved_at = datetime('now') WHERE id = ? AND approved_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn revoke(pool: &SqlitePool, id: i64) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE users SET approved_at = NULL WHERE id = ? AND is_admin = 0",
        )
        .bind(id)
        .execute(pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash failed: {e}"))?
        .to_string())
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// Pre-computed argon2 hash of "dummy" — used to equalise timing when the
// requested email has no user. Regenerate by hashing any constant; the exact
// value doesn't matter, only that argon2 spends its usual time verifying it.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$cGxhY2Vob2xkZXJzYWx0$M7d6ZG2Tq89yYKyVOfITdvOaGkjRpY7UpRCYVzRLmVM";
