use axum::Router;
use monty_server::{Config, app_with_pool, db};
use sqlx::SqlitePool;
use std::sync::Arc;
use tempfile::TempDir;
use tower_sessions::{Expiry, Session};
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

    /// Creates a session via the tower-sessions `Session` API (so the record
    /// is serialized identically to what the middleware expects) and returns
    /// an `id=...` cookie header.
    #[allow(dead_code)]
    pub async fn sign_in(&self, user_id: i64) -> String {
        let store = Arc::new(SqliteStore::new(self.pool.clone()));
        let session = Session::new(None, store, Some(Expiry::OnInactivity(time::Duration::days(7))));
        session.insert("user_id", user_id.to_string()).await.unwrap();
        session.save().await.unwrap();
        let id = session.id().expect("session id assigned after save");
        format!("id={id}")
    }
}
