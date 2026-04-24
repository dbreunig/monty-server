mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::envs::executor::EnvExecutor;
use monty_server::envs::locks::LockRegistry;
use monty_server::envs::storage::EnvStorage;
use monty_server::models::{ApiKey, Env, Function, FunctionCall, User};
use serde_json::Value;
use tower::ServiceExt;

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response was not JSON: {e}; body: {:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
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

fn req_post_json(uri: &str, token: Option<&str>, body: String) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(body)).unwrap()
}

async fn create_stateless_fn(
    app: &TestApp,
    user_id: i64,
    slug: &str,
    code: &str,
) -> Function {
    Function::create(&app.pool, user_id, slug, code, "stateless", None, false, true)
        .await
        .unwrap()
}

#[tokio::test]
async fn authed_call_with_valid_body_returns_handler_result() {
    let (app, user, token) = setup().await;
    let code = "def handler(x: int) -> int:\n    return x * 2\n";
    create_stateless_fn(&app, user.id, "doubler", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            "/fn/doubler",
            Some(&token),
            serde_json::json!({ "x": 21 }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(ct.starts_with("application/json"), "ct was {ct}");
    let bytes = body_bytes(res).await;
    assert_eq!(String::from_utf8(bytes).unwrap(), "42");
}

#[tokio::test]
async fn missing_required_field_returns_400_with_missing() {
    let (app, user, token) = setup().await;
    let code = "def handler(x: int) -> int:\n    return x\n";
    create_stateless_fn(&app, user.id, "needs-x", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            "/fn/needs-x",
            Some(&token),
            "{}".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    let missing = body["missing"].as_array().unwrap();
    assert!(
        missing.iter().any(|v| v == "x"),
        "missing did not include x: {:?}",
        body
    );
}

#[tokio::test]
async fn wrong_type_returns_400_with_wrong_type() {
    let (app, user, token) = setup().await;
    let code = "def handler(x: int) -> int:\n    return x\n";
    create_stateless_fn(&app, user.id, "int-only", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            "/fn/int-only",
            Some(&token),
            serde_json::json!({ "x": "oops" }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    let wrong = body["wrong_type"].as_array().unwrap();
    assert!(
        wrong.iter().any(|v| v == "x"),
        "wrong_type missing x: {:?}",
        body
    );
}

#[tokio::test]
async fn unexpected_key_returns_400_with_unexpected() {
    let (app, user, token) = setup().await;
    let code = "def handler(x: int) -> int:\n    return x\n";
    create_stateless_fn(&app, user.id, "strict", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            "/fn/strict",
            Some(&token),
            serde_json::json!({ "x": 1, "extra": true }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    let unexpected = body["unexpected"].as_array().unwrap();
    assert!(
        unexpected.iter().any(|v| v == "extra"),
        "unexpected missing 'extra': {:?}",
        body
    );
}

#[tokio::test]
async fn malformed_json_body_returns_400() {
    let (app, user, token) = setup().await;
    let code = "def handler() -> int:\n    return 1\n";
    create_stateless_fn(&app, user.id, "ping", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json("/fn/ping", Some(&token), "{".into()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(body["error"], "invalid json");
}

#[tokio::test]
async fn missing_bearer_returns_401_for_private_function() {
    let (app, user, _token) = setup().await;
    let code = "def handler() -> int:\n    return 1\n";
    create_stateless_fn(&app, user.id, "private", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json("/fn/private", None, "{}".into()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_call_works_after_toggling_public() {
    let (app, user, _token) = setup().await;
    let code = "def handler() -> int:\n    return 7\n";
    let f = create_stateless_fn(&app, user.id, "pubfn", code).await;

    Function::set_public(&app.pool, &f.id, true).await.unwrap();
    let fetched = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap();
    let token = fetched.public_token.expect("public_token minted");

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            &format!("/fn/p/{token}"),
            None,
            "{}".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = body_bytes(res).await;
    assert_eq!(String::from_utf8(bytes).unwrap(), "7");
}

#[tokio::test]
async fn rotated_token_stops_old_one_from_working() {
    let (app, user, _token) = setup().await;
    let code = "def handler() -> int:\n    return 3\n";
    let f = create_stateless_fn(&app, user.id, "rotme", code).await;
    Function::set_public(&app.pool, &f.id, true).await.unwrap();
    let old_token = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap()
        .public_token
        .unwrap();

    Function::rotate_public_token(&app.pool, &f.id)
        .await
        .unwrap();
    let new_token = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap()
        .public_token
        .unwrap();
    assert_ne!(old_token, new_token);

    // Old token → 404.
    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            &format!("/fn/p/{old_token}"),
            None,
            "{}".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // New token → 200.
    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            &format!("/fn/p/{new_token}"),
            None,
            "{}".into(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn bound_function_calls_succeed_without_mutating_env() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let (_, token) = ApiKey::create(&app.pool, user.id, "test-key")
        .await
        .unwrap();

    // Create an env and seed it with `value = 100`.
    let env = Env::create(&app.pool, user.id, "e").await.unwrap();
    let storage = EnvStorage::new(app.tmp.path());
    storage.create(&env.id).await.unwrap();
    let exec = monty_server::envs::executor::MontyExecutor::new(
        storage.clone(),
        LockRegistry::new(),
    );
    exec.execute(&env.id, "value = 100").await.unwrap();

    // Create a bound function.
    let code = "def handler() -> int:\n    global value\n    value = value + 1\n    return value\n";
    Function::create(
        &app.pool,
        user.id,
        "bound-fn",
        code,
        "bound",
        Some(&env.id),
        false,
        true,
    )
    .await
    .unwrap();

    // Two calls via HTTP both see value=100 → return 101.
    for _ in 0..2 {
        let res = app
            .router
            .clone()
            .oneshot(req_post_json("/fn/bound-fn", Some(&token), "{}".into()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = body_bytes(res).await;
        assert_eq!(String::from_utf8(bytes).unwrap(), "101");
    }
}

#[tokio::test]
async fn call_logs_are_written() {
    let (app, user, token) = setup().await;
    let code = "def handler(x: int) -> int:\n    return x + 1\n";
    let f = create_stateless_fn(&app, user.id, "logme", code).await;

    let res = app
        .router
        .clone()
        .oneshot(req_post_json(
            "/fn/logme",
            Some(&token),
            serde_json::json!({ "x": 10 }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let logs = FunctionCall::list_recent(&app.pool, &f.id, 10).await.unwrap();
    assert!(!logs.is_empty(), "expected a function_calls row");
    let row = &logs[0];
    assert_eq!(row.status, 200);
    assert_eq!(row.function_id, f.id);
    assert!(row.request_body.as_deref().unwrap().contains("\"x\""));
    assert_eq!(row.response_body.as_deref().unwrap(), "11");
}
