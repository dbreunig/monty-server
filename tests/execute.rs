mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{ApiKey, User};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn make_user_with_token(app: &TestApp) -> (User, String) {
    let user = User::upsert(&app.pool, 1, "u", None).await.unwrap();
    let (_, token) = ApiKey::create(&app.pool, user.id, "t").await.unwrap();
    (user, token)
}

async fn create_env(app: &TestApp, token: &str, name: &str) -> String {
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/envs")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": name }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    body_json(res).await["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn execute_endpoint_runs_stateful_code() {
    let app = TestApp::new().await;
    let (_user, token) = make_user_with_token(&app).await;
    let env_id = create_env(&app, &token, "scratch").await;

    // First call: set x
    let r1 = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/envs/{env_id}/execute"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "code": "x = 40 + 2" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    // Second call: print x
    let r2 = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/envs/{env_id}/execute"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "code": "print(x)" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let body = body_json(r2).await;
    assert_eq!(body["stdout"].as_str().unwrap().trim(), "42");
}

#[tokio::test]
async fn one_off_execute_endpoint_works() {
    let app = TestApp::new().await;
    let (_user, token) = make_user_with_token(&app).await;

    let r = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/execute")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "code": "print(2 + 2)" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = body_json(r).await;
    assert_eq!(body["stdout"].as_str().unwrap().trim(), "4");
}

#[tokio::test]
async fn execute_on_someone_elses_env_is_404() {
    let app = TestApp::new().await;
    let (_alice, alice_token) = make_user_with_token(&app).await;
    let env_id = create_env(&app, &alice_token, "alice").await;

    let bob = User::upsert(&app.pool, 2, "bob", None).await.unwrap();
    let (_, bob_token) = ApiKey::create(&app.pool, bob.id, "t").await.unwrap();

    let r = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/envs/{env_id}/execute"))
                .header("authorization", format!("Bearer {bob_token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "code": "1" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
