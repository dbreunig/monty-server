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
}
