mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::User;
use tower::ServiceExt;

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn urlencoded(form: &[(&str, &str)]) -> String {
    form.iter()
        .map(|(k, v)| {
            let mut s = String::new();
            for c in k.chars() {
                s.push(c);
            }
            s.push('=');
            for b in v.bytes() {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        s.push(b as char);
                    }
                    b' ' => s.push('+'),
                    _ => s.push_str(&format!("%{b:02X}")),
                }
            }
            s
        })
        .collect::<Vec<_>>()
        .join("&")
}

async fn post_form(app: &TestApp, uri: &str, form: &[(&str, &str)]) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(urlencoded(form)))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn signup_creates_pending_user() {
    let app = TestApp::new().await;
    let res = post_form(
        &app,
        "/auth/signup",
        &[("email", "alice@example.com"), ("password", "hunter2hunter")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("approve"), "expected approval notice: {body}");

    let user = User::find_local_by_email(&app.pool, "alice@example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(!user.is_approved(), "new user must start pending");
    assert!(!user.is_admin);
}

#[tokio::test]
async fn duplicate_signup_is_rejected() {
    let app = TestApp::new().await;
    User::create_local(&app.pool, "dup@example.com", "hunter2hunter")
        .await
        .unwrap();
    let res = post_form(
        &app,
        "/auth/signup",
        &[("email", "dup@example.com"), ("password", "hunter2hunter")],
    )
    .await;
    let body = body_string(res).await;
    assert!(body.to_lowercase().contains("already exists"), "{body}");
}

#[tokio::test]
async fn pending_user_cannot_sign_in_and_create_api_keys() {
    let app = TestApp::new().await;
    // Bypass the UI signup path; create a pending user directly.
    let user = User::create_local(&app.pool, "pending@example.com", "hunter2hunter")
        .await
        .unwrap();

    // Sign in works (we get a session cookie) but the create-api-key route
    // should 403 because the user isn't approved.
    let cookie = app.sign_in(user.id).await;
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
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn approved_user_can_create_api_keys() {
    let app = TestApp::new().await;
    let user = User::create_local(&app.pool, "alice@example.com", "hunter2hunter")
        .await
        .unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

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
}

#[tokio::test]
async fn login_with_correct_password_sets_session() {
    let app = TestApp::new().await;
    let user = User::create_local(&app.pool, "alice@example.com", "hunter2hunter")
        .await
        .unwrap();
    User::approve(&app.pool, user.id).await.unwrap();

    let res = post_form(
        &app,
        "/auth/login",
        &[
            ("email", "alice@example.com"),
            ("password", "hunter2hunter"),
        ],
    )
    .await;
    assert_eq!(res.status(), StatusCode::FOUND);
    assert_eq!(res.headers().get("location").unwrap(), "/dashboard");
    assert!(res.headers().get("set-cookie").is_some());
}

#[tokio::test]
async fn login_with_wrong_password_renders_error() {
    let app = TestApp::new().await;
    User::create_local(&app.pool, "alice@example.com", "hunter2hunter")
        .await
        .unwrap();
    let res = post_form(
        &app,
        "/auth/login",
        &[("email", "alice@example.com"), ("password", "wrongwrongwrong")],
    )
    .await;
    let body = body_string(res).await;
    assert!(body.to_lowercase().contains("invalid"), "{body}");
}

#[tokio::test]
async fn admin_can_approve_pending_user() {
    let app = TestApp::new().await;
    // Seed admin
    let admin = User::ensure_admin(&app.pool, "admin@example.com", "superhunter2")
        .await
        .unwrap();
    assert!(admin.is_admin);
    assert!(admin.is_approved());

    // Create a pending user
    let pending = User::create_local(&app.pool, "bob@example.com", "hunter2hunter")
        .await
        .unwrap();
    assert!(!pending.is_approved());

    // Admin signs in and approves
    let cookie = app.sign_in(admin.id).await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/admin/approve/{}", pending.id))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let refreshed = User::find_by_id(&app.pool, pending.id)
        .await
        .unwrap()
        .unwrap();
    assert!(refreshed.is_approved());
}

#[tokio::test]
async fn non_admin_cannot_access_admin_panel() {
    let app = TestApp::new().await;
    let user = app.make_approved_user(1, "alice").await;
    let cookie = app.sign_in(user.id).await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bearer_token_for_pending_user_is_403() {
    let app = TestApp::new().await;
    let user = User::create_local(&app.pool, "alice@example.com", "hunter2hunter")
        .await
        .unwrap();
    let (_, token) = monty_server::models::ApiKey::create(&app.pool, user.id, "t")
        .await
        .unwrap();

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/envs")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
