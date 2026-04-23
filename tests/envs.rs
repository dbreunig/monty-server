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

async fn make_user_with_token(app: &TestApp, github_id: i64, name: &str) -> (User, String) {
    let user = User::upsert(&app.pool, github_id, name, None).await.unwrap();
    let (_, token) = ApiKey::create(&app.pool, user.id, "t").await.unwrap();
    (user, token)
}

fn post_json(uri: &str, body: Value, bearer: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn create_env_via_api() {
    let app = TestApp::new().await;
    let (_user, token) = make_user_with_token(&app, 1, "u").await;

    let res = app
        .router
        .clone()
        .oneshot(post_json(
            "/api/v1/envs",
            json!({ "name": "scratch" }),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"], "scratch");
    assert!(app.tmp.path().join("envs").join(&id).join("files").is_dir());
    assert!(
        app.tmp
            .path()
            .join("envs")
            .join(&id)
            .join("snapshot.bin")
            .is_file()
    );
}

#[tokio::test]
async fn list_envs_returns_only_own() {
    let app = TestApp::new().await;
    let (_, alice_token) = make_user_with_token(&app, 1, "alice").await;
    let (_, bob_token) = make_user_with_token(&app, 2, "bob").await;

    app.router
        .clone()
        .oneshot(post_json(
            "/api/v1/envs",
            json!({ "name": "alice-env" }),
            &alice_token,
        ))
        .await
        .unwrap();
    app.router
        .clone()
        .oneshot(post_json(
            "/api/v1/envs",
            json!({ "name": "bob-env" }),
            &bob_token,
        ))
        .await
        .unwrap();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/envs")
                .header("authorization", format!("Bearer {alice_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(res).await;
    let envs = body.as_array().unwrap();
    assert_eq!(envs.len(), 1);
    assert_eq!(envs[0]["name"], "alice-env");
}

#[tokio::test]
async fn cannot_delete_other_users_env() {
    let app = TestApp::new().await;
    let (_, alice_token) = make_user_with_token(&app, 1, "alice").await;
    let (_, bob_token) = make_user_with_token(&app, 2, "bob").await;

    let created = app
        .router
        .clone()
        .oneshot(post_json(
            "/api/v1/envs",
            json!({ "name": "alice-env" }),
            &alice_token,
        ))
        .await
        .unwrap();
    let body = body_json(created).await;
    let id = body["id"].as_str().unwrap().to_string();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/envs/{id}"))
                .header("authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert!(app.tmp.path().join("envs").join(&id).exists());
}

#[tokio::test]
async fn delete_env_removes_row_and_directory() {
    let app = TestApp::new().await;
    let (_, token) = make_user_with_token(&app, 1, "u").await;

    let created = app
        .router
        .clone()
        .oneshot(post_json(
            "/api/v1/envs",
            json!({ "name": "throwaway" }),
            &token,
        ))
        .await
        .unwrap();
    let body = body_json(created).await;
    let id = body["id"].as_str().unwrap().to_string();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/envs/{id}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(!app.tmp.path().join("envs").join(&id).exists());
}
