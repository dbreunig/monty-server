use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Function {
    pub id: String,
    pub user_id: i64,
    pub slug: String,
    pub code: String,
    pub mode: String,
    pub env_id: Option<String>,
    pub is_public: bool,
    pub public_token: Option<String>,
    pub truncate_logs: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Validates a function slug: `^[a-z0-9][a-z0-9_-]{0,62}$`.
/// - First char: lowercase alphanumeric.
/// - Remaining chars (0-62): lowercase alphanumeric, `_`, or `-`.
/// - Total length: 1-63.
pub fn validate_slug(s: &str) -> Result<(), anyhow::Error> {
    if s.is_empty() {
        return Err(anyhow!("slug must not be empty"));
    }
    if s.len() > 63 {
        return Err(anyhow!("slug must be at most 63 characters"));
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(anyhow!(
            "slug must start with a lowercase letter or digit"
        ));
    }
    for c in chars {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if !ok {
            return Err(anyhow!(
                "slug may only contain lowercase letters, digits, '_' and '-'"
            ));
        }
    }
    Ok(())
}

fn mint_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

impl Function {
    pub fn new_id() -> String {
        Uuid::new_v4().simple().to_string()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        pool: &SqlitePool,
        user_id: i64,
        slug: &str,
        code: &str,
        mode: &str,
        env_id: Option<&str>,
        is_public: bool,
        truncate_logs: bool,
    ) -> Result<Self> {
        validate_slug(slug)?;
        let id = Self::new_id();
        let public_token = if is_public { Some(mint_token()) } else { None };
        let row = sqlx::query_as::<_, Function>(
            r#"
            INSERT INTO functions
                (id, user_id, slug, code, mode, env_id, is_public, public_token, truncate_logs)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING id, user_id, slug, code, mode, env_id, is_public, public_token,
                      truncate_logs, created_at, updated_at
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(slug)
        .bind(code)
        .bind(mode)
        .bind(env_id)
        .bind(is_public)
        .bind(public_token.as_deref())
        .bind(truncate_logs)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_for_user(
        pool: &SqlitePool,
        user_id: i64,
        id: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Function>(
            r#"
            SELECT id, user_id, slug, code, mode, env_id, is_public, public_token,
                   truncate_logs, created_at, updated_at
            FROM functions WHERE user_id = ? AND id = ?
            "#,
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_by_slug_for_user(
        pool: &SqlitePool,
        user_id: i64,
        slug: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Function>(
            r#"
            SELECT id, user_id, slug, code, mode, env_id, is_public, public_token,
                   truncate_logs, created_at, updated_at
            FROM functions WHERE user_id = ? AND slug = ?
            "#,
        )
        .bind(user_id)
        .bind(slug)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn find_by_public_token(
        pool: &SqlitePool,
        token: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Function>(
            r#"
            SELECT id, user_id, slug, code, mode, env_id, is_public, public_token,
                   truncate_logs, created_at, updated_at
            FROM functions WHERE public_token = ?
            "#,
        )
        .bind(token)
        .fetch_optional(pool)
        .await?;
        Ok(row)
    }

    pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Function>(
            r#"
            SELECT id, user_id, slug, code, mode, env_id, is_public, public_token,
                   truncate_logs, created_at, updated_at
            FROM functions WHERE user_id = ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    pub async fn update_code(pool: &SqlitePool, id: &str, new_code: &str) -> Result<()> {
        sqlx::query(
            "UPDATE functions SET code = ?, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(new_code)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Toggles `is_public`. When flipping 0->1, mints a fresh token; when
    /// flipping 1->0, clears the token. No-op (no token rotation) if already
    /// in the target state.
    pub async fn set_public(pool: &SqlitePool, id: &str, is_public: bool) -> Result<()> {
        let current: Option<bool> =
            sqlx::query_scalar("SELECT is_public FROM functions WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some(current) = current else {
            return Err(anyhow!("function not found"));
        };
        if current == is_public {
            return Ok(());
        }
        if is_public {
            let token = mint_token();
            sqlx::query(
                r#"
                UPDATE functions
                SET is_public = 1, public_token = ?, updated_at = datetime('now')
                WHERE id = ?
                "#,
            )
            .bind(&token)
            .bind(id)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                r#"
                UPDATE functions
                SET is_public = 0, public_token = NULL, updated_at = datetime('now')
                WHERE id = ?
                "#,
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    /// Mints a fresh public token. Errors if the function is not currently
    /// public.
    pub async fn rotate_public_token(pool: &SqlitePool, id: &str) -> Result<()> {
        let current: Option<bool> =
            sqlx::query_scalar("SELECT is_public FROM functions WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some(is_public) = current else {
            return Err(anyhow!("function not found"));
        };
        if !is_public {
            return Err(anyhow!("function is not public"));
        }
        let token = mint_token();
        sqlx::query(
            r#"
            UPDATE functions
            SET public_token = ?, updated_at = datetime('now')
            WHERE id = ?
            "#,
        )
        .bind(&token)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn delete_for_user(
        pool: &SqlitePool,
        user_id: i64,
        id: &str,
    ) -> Result<bool> {
        let res = sqlx::query("DELETE FROM functions WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn touch(pool: &SqlitePool, id: &str) -> Result<()> {
        sqlx::query("UPDATE functions SET updated_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::User;

    async fn pool() -> sqlx::SqlitePool {
        let pool = db::connect("sqlite::memory:").await.unwrap();
        db::migrate(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn create_and_fetch() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 42, "u", None).await.unwrap();
        let f = Function::create(
            &pool,
            user.id,
            "hello",
            "def handler(): return 1",
            "stateless",
            None,
            false,
            true,
        )
        .await
        .unwrap();
        let fetched = Function::find_by_slug_for_user(&pool, user.id, "hello")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.id, f.id);
        assert_eq!(fetched.code, "def handler(): return 1");
    }

    #[tokio::test]
    async fn publishing_mints_token_unpublishing_clears_it() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 1, "u", None).await.unwrap();
        let f = Function::create(
            &pool,
            user.id,
            "pub",
            "def handler(): ...",
            "stateless",
            None,
            false,
            true,
        )
        .await
        .unwrap();
        Function::set_public(&pool, &f.id, true).await.unwrap();
        let f2 = Function::find_for_user(&pool, user.id, &f.id)
            .await
            .unwrap()
            .unwrap();
        assert!(f2.is_public);
        assert!(f2.public_token.is_some());
        let tok = f2.public_token.clone().unwrap();
        Function::set_public(&pool, &f.id, false).await.unwrap();
        let f3 = Function::find_for_user(&pool, user.id, &f.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!f3.is_public);
        assert!(f3.public_token.is_none());
        // New token after re-publish.
        Function::set_public(&pool, &f.id, true).await.unwrap();
        let f4 = Function::find_for_user(&pool, user.id, &f.id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(f4.public_token.unwrap(), tok);
    }

    #[tokio::test]
    async fn set_public_is_idempotent_and_does_not_rotate() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 7, "u", None).await.unwrap();
        let f = Function::create(
            &pool,
            user.id,
            "idem",
            "def handler(): ...",
            "stateless",
            None,
            true,
            true,
        )
        .await
        .unwrap();
        // Already public — calling set_public(true) again must not rotate.
        let tok_before = f.public_token.clone().unwrap();
        Function::set_public(&pool, &f.id, true).await.unwrap();
        let f2 = Function::find_for_user(&pool, user.id, &f.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f2.public_token.unwrap(), tok_before);
    }

    #[tokio::test]
    async fn rotate_fails_when_private() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 2, "u", None).await.unwrap();
        let f = Function::create(
            &pool,
            user.id,
            "priv",
            "def handler(): ...",
            "stateless",
            None,
            false,
            true,
        )
        .await
        .unwrap();
        let err = Function::rotate_public_token(&pool, &f.id).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn rotate_changes_token_when_public() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 3, "u", None).await.unwrap();
        let f = Function::create(
            &pool,
            user.id,
            "rot",
            "def handler(): ...",
            "stateless",
            None,
            true,
            true,
        )
        .await
        .unwrap();
        let before = f.public_token.clone().unwrap();
        Function::rotate_public_token(&pool, &f.id).await.unwrap();
        let after = Function::find_for_user(&pool, user.id, &f.id)
            .await
            .unwrap()
            .unwrap()
            .public_token
            .unwrap();
        assert_ne!(before, after);
    }

    #[tokio::test]
    async fn delete_respects_ownership() {
        let pool = pool().await;
        let owner = User::upsert_github(&pool, 10, "owner", None).await.unwrap();
        let other = User::upsert_github(&pool, 11, "other", None).await.unwrap();
        let f = Function::create(
            &pool,
            owner.id,
            "owned",
            "def handler(): ...",
            "stateless",
            None,
            false,
            true,
        )
        .await
        .unwrap();
        // Wrong user can't delete — should return false.
        let deleted_wrong = Function::delete_for_user(&pool, other.id, &f.id)
            .await
            .unwrap();
        assert!(!deleted_wrong);
        // Real owner succeeds.
        let deleted_right = Function::delete_for_user(&pool, owner.id, &f.id)
            .await
            .unwrap();
        assert!(deleted_right);
    }

    #[test]
    fn validate_slug_accepts_good_and_rejects_bad() {
        // Good.
        assert!(validate_slug("a").is_ok());
        assert!(validate_slug("hello").is_ok());
        assert!(validate_slug("my-func_2").is_ok());
        assert!(validate_slug("0start-ok").is_ok());
        assert!(validate_slug(&"a".repeat(63)).is_ok());
        // Bad.
        assert!(validate_slug("").is_err());
        assert!(validate_slug("-leading-dash").is_err());
        assert!(validate_slug("_leading-underscore").is_err());
        assert!(validate_slug("Has-Upper").is_err());
        assert!(validate_slug("has space").is_err());
        assert!(validate_slug("has.dot").is_err());
        assert!(validate_slug(&"a".repeat(64)).is_err());
    }

    #[tokio::test]
    async fn create_rejects_invalid_slug() {
        let pool = pool().await;
        let user = User::upsert_github(&pool, 99, "u", None).await.unwrap();
        let res = Function::create(
            &pool,
            user.id,
            "Bad Slug",
            "def handler(): ...",
            "stateless",
            None,
            false,
            true,
        )
        .await;
        assert!(res.is_err());
    }
}
