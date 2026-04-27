mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{Env, User};
use tower::ServiceExt;

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn root_redirects_to_login_when_signed_out() {
    let app = TestApp::new().await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/auth/login");
}

#[tokio::test]
async fn root_redirects_to_dashboard_when_signed_in() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/dashboard");
}

#[tokio::test]
async fn dashboard_lists_environments() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    Env::create(&app.pool, user.id, "scratch").await.unwrap();
    let cookie = app.sign_in(user.id).await;

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
    assert!(body.contains("Environments"));
    assert!(body.contains("scratch"));
}

#[tokio::test]
async fn env_detail_page_shows_name_and_files() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let env = Env::create(&app.pool, user.id, "scratch").await.unwrap();
    // Ensure storage dir exists
    monty_server::envs::storage::EnvStorage::new(app.tmp.path())
        .create(&env.id)
        .await
        .unwrap();
    std::fs::write(
        app.tmp
            .path()
            .join("envs")
            .join(&env.id)
            .join("files")
            .join("data.txt"),
        b"hello",
    )
    .unwrap();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/envs/{}", env.id))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("scratch"));
    assert!(body.contains("data.txt"));
}

#[tokio::test]
async fn env_detail_for_other_user_is_404() {
    let app = TestApp::new().await;
    let alice = User::upsert_github(&app.pool, 1, "alice", None).await.unwrap();
    User::approve(&app.pool, alice.id).await.unwrap();
    let bob = User::upsert_github(&app.pool, 2, "bob", None).await.unwrap();
    User::approve(&app.pool, bob.id).await.unwrap();
    let env = Env::create(&app.pool, alice.id, "alice-env").await.unwrap();
    let cookie = app.sign_in(bob.id).await;

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/envs/{}", env.id))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
