mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{ApiKey, User};
use tower::ServiceExt;

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn api_key_create_list_delete_round_trip() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

    // Create
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api-keys")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=laptop"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("mk_"), "must display plaintext token once");
    assert!(body.contains("laptop"));

    // Capture the plaintext so we can confirm it's NOT present in the dashboard
    let plaintext = body
        .split("mk_")
        .nth(1)
        .and_then(|s| s.split('<').next())
        .map(|s| format!("mk_{}", s.trim()))
        .unwrap_or_default();
    assert!(plaintext.starts_with("mk_"));

    // List
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("laptop"), "key name must render");
    assert!(
        !body.contains(&plaintext),
        "plaintext must NOT reappear on dashboard"
    );

    // Delete
    let keys = ApiKey::list_for_user(&app.pool, user.id).await.unwrap();
    assert_eq!(keys.len(), 1);
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api-keys/{}/delete", keys[0].id))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        ApiKey::list_for_user(&app.pool, user.id)
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn dashboard_requires_session() {
    let app = TestApp::new().await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
