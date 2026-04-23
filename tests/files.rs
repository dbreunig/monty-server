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

async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

async fn setup() -> (TestApp, String, String) {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let (_, token) = ApiKey::create(&app.pool, user.id, "t").await.unwrap();
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/envs")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "scratch" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let env_id = body_json(res).await["id"].as_str().unwrap().to_string();
    (app, token, env_id)
}

fn multipart_body(boundary: &str, filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn upload_list_download_delete_roundtrip() {
    let (app, token, env_id) = setup().await;
    let boundary = "X-BOUNDARY";
    let body = multipart_body(boundary, "hello.txt", b"hi there");

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/envs/{env_id}/files"))
                .header("authorization", format!("Bearer {token}"))
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/envs/{env_id}/files"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let files = body_json(res).await;
    assert_eq!(files[0]["name"], "hello.txt");
    assert_eq!(files[0]["size"], 8);

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/envs/{env_id}/files/hello.txt"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_bytes(res).await, b"hi there");

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/envs/{env_id}/files/hello.txt"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn path_traversal_in_filename_is_rejected() {
    let (app, token, env_id) = setup().await;

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/envs/{env_id}/files/..%2F..%2Fetc%2Fpasswd"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Either the router refuses or the handler rejects; both are acceptable.
    assert!(res.status().is_client_error() || res.status().is_server_error());
}

#[tokio::test]
async fn cannot_touch_other_users_env_files() {
    let (app, _alice_token, env_id) = setup().await;
    let bob = User::upsert_github(&app.pool, 2, "bob", None).await.unwrap();
    User::approve(&app.pool, bob.id).await.unwrap();
    let (_, bob_token) = ApiKey::create(&app.pool, bob.id, "t").await.unwrap();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/envs/{env_id}/files"))
                .header("authorization", format!("Bearer {bob_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
