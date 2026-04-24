mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{ApiKey, Env, User};
use serde_json::Value;
use tower::ServiceExt;

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("valid json")
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Sets up a TestApp, an approved user, and a bearer API key plaintext.
async fn setup() -> (TestApp, User, String) {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let (_, plaintext) = ApiKey::create(&app.pool, user.id, "test-key")
        .await
        .unwrap();
    (app, user, plaintext)
}

fn req_json(
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn req_empty(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn create_list_get_delete_roundtrip() {
    let (app, _user, token) = setup().await;

    // Create
    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &token,
            serde_json::json!({
                "slug": "hello",
                "code": "def handler(name: str):\n    return name",
                "mode": "stateless",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created = body_json(res).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["slug"], "hello");
    assert_eq!(created["mode"], "stateless");
    assert_eq!(created["is_public"], false);
    assert_eq!(created["truncate_logs"], true);

    // List
    let res = app
        .router
        .clone()
        .oneshot(req_empty("GET", "/api/v1/functions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list = body_json(res).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);

    // Get
    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "GET",
            &format!("/api/v1/functions/{id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let got = body_json(res).await;
    assert_eq!(got["id"], id);
    assert_eq!(got["slug"], "hello");

    // Delete
    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "DELETE",
            &format!("/api/v1/functions/{id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Confirm deletion
    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "GET",
            &format!("/api/v1/functions/{id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn slug_collision_returns_409() {
    let (app, _user, token) = setup().await;

    let payload = serde_json::json!({
        "slug": "dup",
        "code": "def handler(): return 1",
        "mode": "stateless",
    });

    let res = app
        .router
        .clone()
        .oneshot(req_json("POST", "/api/v1/functions", &token, payload.clone()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .router
        .clone()
        .oneshot(req_json("POST", "/api/v1/functions", &token, payload))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = body_json(res).await;
    assert_eq!(body["error"], "slug already in use");
}

#[tokio::test]
async fn bound_mode_requires_env_id_returns_400() {
    let (app, _user, token) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &token,
            serde_json::json!({
                "slug": "bound-nope",
                "code": "def handler(): return 1",
                "mode": "bound",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("env_id"),
        "error was {:?}",
        body["error"]
    );
}

#[tokio::test]
async fn bound_mode_with_foreign_env_returns_400() {
    let app = TestApp::new().await;
    let alice = User::upsert_github(&app.pool, 1, "alice", None)
        .await
        .unwrap();
    User::approve(&app.pool, alice.id).await.unwrap();
    let bob = User::upsert_github(&app.pool, 2, "bob", None).await.unwrap();
    User::approve(&app.pool, bob.id).await.unwrap();

    // env owned by bob
    let bob_env = Env::create(&app.pool, bob.id, "bob-env").await.unwrap();

    let (_, alice_token) = ApiKey::create(&app.pool, alice.id, "alice-key")
        .await
        .unwrap();

    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &alice_token,
            serde_json::json!({
                "slug": "cross",
                "code": "def handler(): return 1",
                "mode": "bound",
                "env_id": bob_env.id,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("env_id")
    );
}

#[tokio::test]
async fn invalid_code_returns_400() {
    let (app, _user, token) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &token,
            serde_json::json!({
                "slug": "badcode",
                "code": "def other(): return 1",
                "mode": "stateless",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(body["error"], "invalid code");
    assert!(!body["reason"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn patch_code_updates_function() {
    let (app, _user, token) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &token,
            serde_json::json!({
                "slug": "upd",
                "code": "def handler(): return 1",
                "mode": "stateless",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created = body_json(res).await;
    let id = created["id"].as_str().unwrap().to_string();

    let new_code = "def handler(name: str):\n    return name.upper()";
    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "PATCH",
            &format!("/api/v1/functions/{id}"),
            &token,
            serde_json::json!({ "code": new_code }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated = body_json(res).await;
    assert_eq!(updated["code"], new_code);

    // Rejects invalid code on patch.
    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "PATCH",
            &format!("/api/v1/functions/{id}"),
            &token,
            serde_json::json!({ "code": "def nothandler(): return 1" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn toggle_public_and_rotate() {
    let (app, _user, token) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "POST",
            "/api/v1/functions",
            &token,
            serde_json::json!({
                "slug": "pubtest",
                "code": "def handler(): return 1",
                "mode": "stateless",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created = body_json(res).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert!(created["public_token"].is_null());

    // Flip to public.
    let res = app
        .router
        .clone()
        .oneshot(req_json(
            "PATCH",
            &format!("/api/v1/functions/{id}"),
            &token,
            serde_json::json!({ "is_public": true }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated = body_json(res).await;
    assert_eq!(updated["is_public"], true);
    let first_token = updated["public_token"]
        .as_str()
        .expect("public_token present")
        .to_string();
    assert!(!first_token.is_empty());

    // Rotate.
    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "POST",
            &format!("/api/v1/functions/{id}/rotate"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let rotated = body_json(res).await;
    let second_token = rotated["public_token"].as_str().unwrap().to_string();
    assert_ne!(first_token, second_token);

    // Rotating while private errors with 409.
    let _ = app
        .router
        .clone()
        .oneshot(req_json(
            "PATCH",
            &format!("/api/v1/functions/{id}"),
            &token,
            serde_json::json!({ "is_public": false }),
        ))
        .await
        .unwrap();
    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "POST",
            &format!("/api/v1/functions/{id}/rotate"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = body_string(res).await;
    assert!(body.to_lowercase().contains("not public"));
}

#[tokio::test]
async fn delete_nonexistent_returns_404() {
    let (app, _user, token) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(req_empty(
            "DELETE",
            "/api/v1/functions/00000000000000000000000000000000",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
