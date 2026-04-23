use axum::Router;
use monty_server::{Config, app_with_pool, db};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower_sessions::SessionStore;
use tower_sessions::session::{Id, Record};
use tower_sessions_sqlx_store::SqliteStore;

#[allow(dead_code)]
pub struct TestApp {
    pub router: Router,
    pub pool: SqlitePool,
    pub tmp: TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config::for_test(tmp.path().to_path_buf());
        let pool = db::connect(":memory:").await.unwrap();
        let router = app_with_pool(config, pool.clone()).await.unwrap();
        Self { router, pool, tmp }
    }

    #[allow(dead_code)]
    pub async fn new_with_github_mock(github_base: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(tmp.path().to_path_buf());
        config.github_token_url = format!("{github_base}/login/oauth/access_token");
        config.github_api_base = github_base.to_string();
        let pool = db::connect(":memory:").await.unwrap();
        let router = app_with_pool(config, pool.clone()).await.unwrap();
        Self { router, pool, tmp }
    }

    /// Creates a session record with `user_id` populated and returns a
    /// `cookie: id=...` header value. Bypasses the OAuth flow.
    #[allow(dead_code)]
    pub async fn sign_in(&self, user_id: i64) -> String {
        let store = SqliteStore::new(self.pool.clone());
        let mut data = std::collections::HashMap::new();
        data.insert(
            "user_id".to_string(),
            serde_json::to_value(user_id).unwrap(),
        );
        let mut record = Record {
            id: Id::default(),
            data,
            expiry_date: time::OffsetDateTime::now_utc() + time::Duration::days(7),
        };
        store.create(&mut record).await.unwrap();
        format!("id={}", record.id)
    }
}
