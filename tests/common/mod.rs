use axum::Router;
use monty_server::app;

pub struct TestApp {
    pub router: Router,
}

impl TestApp {
    pub async fn new() -> Self {
        Self { router: app().await }
    }
}
