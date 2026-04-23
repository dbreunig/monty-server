use axum::Router;
use monty_server::{Config, app_with_pool, db};
use sqlx::SqlitePool;
use tempfile::TempDir;

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

    pub async fn new_with_github_mock(github_base: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = Config::for_test(tmp.path().to_path_buf());
        config.github_token_url = format!("{github_base}/login/oauth/access_token");
        config.github_api_base = github_base.to_string();
        let pool = db::connect(":memory:").await.unwrap();
        let router = app_with_pool(config, pool.clone()).await.unwrap();
        Self { router, pool, tmp }
    }
}
