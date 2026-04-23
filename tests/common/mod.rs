use axum::Router;
use monty_server::{Config, app};
use tempfile::TempDir;

pub struct TestApp {
    pub router: Router,
    #[allow(dead_code)]
    pub tmp: TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config = Config::for_test(tmp.path().to_path_buf());
        let router = app(config).await;
        Self { router, tmp }
    }
}
