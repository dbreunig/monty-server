mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use monty_server::models::{ApiKey, User};
use tower::ServiceExt;

#[tokio::test]
async fn login_redirects_to_github_with_state_cookie() {
    let app = TestApp::new().await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login-github")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    let loc = res.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.starts_with("https://github.com/login/oauth/authorize?"),
        "unexpected redirect: {loc}"
    );
    assert!(loc.contains("state="));
    // Session cookie set so we can verify the state in callback.
    assert!(res.headers().get("set-cookie").is_some());
}

#[tokio::test]
async fn callback_completes_oauth_with_mocked_github() {
    use monty_server::models::User;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "gho_x",
            "token_type": "bearer",
            "scope": "read:user user:email"
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": 42,
            "login": "octocat"
        })))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/user/emails"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"email": "o@example.com", "primary": true, "verified": true}
        ])))
        .mount(&mock)
        .await;

    let app = TestApp::new_with_github_mock(&mock.uri()).await;

    // First hit /auth/login to seed the session state.
    let login_res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login-github")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login_res
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let loc = login_res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = loc
        .split_once("state=")
        .unwrap()
        .1
        .split('&')
        .next()
        .unwrap()
        .to_string();

    // Now hit the callback with the same cookie and state.
    let cb = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/auth/callback?code=x&state={state}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cb.status(), StatusCode::FOUND, "expected redirect");
    assert_eq!(
        cb.headers().get("location").unwrap().to_str().unwrap(),
        "/dashboard"
    );

    let found = User::find_by_github_id(&app.pool, 42)
        .await
        .unwrap()
        .expect("user row created");
    assert_eq!(found.github_login.as_deref(), Some("octocat"));
    assert_eq!(found.email.as_deref(), Some("o@example.com"));
}

#[tokio::test]
async fn bearer_token_auth_accepts_valid_rejects_invalid() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let (_, token) = ApiKey::create(&app.pool, user.id, "t").await.unwrap();

    // Valid
    let r = app
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
    assert_eq!(r.status(), 200);

    // Invalid
    let r = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/envs")
                .header("authorization", "Bearer mk_nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Missing
    let r = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/envs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}
