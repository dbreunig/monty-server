mod common;
use axum::{body::Body, http::Request};
use common::TestApp;
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_ok() {
    let app = TestApp::new().await;
    let res = app
        .router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}
