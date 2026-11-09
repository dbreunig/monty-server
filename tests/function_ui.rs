mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{ApiKey, Env, Function, User};
use tower::ServiceExt;

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get_dashboard(app: &TestApp, cookie: &str) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn post_form(
    app: &TestApp,
    cookie: &str,
    uri: &str,
    body: &str,
) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn dashboard_shows_functions_section() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    Function::create(
        &app.pool,
        user.id,
        "my-func",
        "def handler(): return 1",
        "stateless",
        None,
        false,
        true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    let res = get_dashboard(&app, &cookie).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("Functions"), "dashboard should have Functions section");
    assert!(body.contains("my-func"), "dashboard should list user's function");
    assert!(
        body.contains("action=\"/functions\""),
        "dashboard should include the new-function form"
    );
    assert!(body.contains("name=\"slug\""));
    assert!(body.contains("name=\"target\""));
    assert!(body.contains("name=\"code\""));
}

#[tokio::test]
async fn create_function_via_form_redirects_to_dashboard() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

    // URL-encoded form body. `code` = "def handler():\n    return 1\n"
    let form = "slug=webhook&target=stateless&code=def+handler%28%29%3A%0A++++return+1%0A";
    let res = post_form(&app, &cookie, "/functions", form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/dashboard");

    // Verify the new function appears on the dashboard.
    let res = get_dashboard(&app, &cookie).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("webhook"));
}

#[tokio::test]
async fn create_with_foreign_env_renders_error() {
    let app = TestApp::new().await;
    let alice = User::upsert_github(&app.pool, 1, "alice", None).await.unwrap();
    User::approve(&app.pool, alice.id).await.unwrap();
    let bob = User::upsert_github(&app.pool, 2, "bob", None).await.unwrap();
    User::approve(&app.pool, bob.id).await.unwrap();
    let bob_env = Env::create(&app.pool, bob.id, "bobs-env").await.unwrap();
    let cookie = app.sign_in(alice.id).await;

    // Alice tries to bind a function to Bob's env.
    let form = format!(
        "slug=stolen&target={}&code=def+handler%28%29%3A+return+1",
        bob_env.id
    );
    let res = post_form(&app, &cookie, "/functions", &form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with("/dashboard?error="),
        "expected redirect to /dashboard?error=..., got {location}"
    );

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&location)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(
        body.contains("environment does not exist"),
        "dashboard should surface the validation error; body: {body}"
    );

    let list = Function::list_for_user(&app.pool, alice.id).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn delete_function_via_form() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let f = Function::create(
        &app.pool,
        user.id,
        "doomed",
        "def handler(): return 1",
        "stateless",
        None,
        false,
        true,
    )
    .await
    .unwrap();

    let res = post_form(
        &app,
        &cookie,
        &format!("/functions/{}/delete", f.id),
        "",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/dashboard");

    let list = Function::list_for_user(&app.pool, user.id).await.unwrap();
    assert!(list.is_empty(), "function should be gone after delete");
}

#[tokio::test]
async fn create_function_bound_with_valid_env_succeeds() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let env = Env::create(&app.pool, user.id, "prod").await.unwrap();
    let cookie = app.sign_in(user.id).await;

    let form = format!(
        "slug=boundfn&target={}&code=def+handler%28%29%3A+return+1",
        env.id
    );
    let res = post_form(&app, &cookie, "/functions", &form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/dashboard");

    let fetched = Function::find_by_slug_for_user(&app.pool, user.id, "boundfn")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.mode, "bound");
    assert_eq!(fetched.env_id.as_deref(), Some(env.id.as_str()));
}

async fn get_path(app: &TestApp, cookie: &str, uri: &str) -> axum::response::Response {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn detail_page_shows_code_and_call_url() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let code = "def handler(name: str) -> dict:\n    return {\"hello\": name}\n";
    let f = Function::create(
        &app.pool, user.id, "greet", code, "stateless", None, false, true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    let res = get_path(&app, &cookie, &format!("/functions/{}", f.id)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    // Code shows up, escaped-as-HTML-safe rendering of the python source.
    assert!(body.contains("def handler(name: str)"), "body should contain code; got: {body}");
    // Authed call URL (private function).
    assert!(body.contains("/fn/greet"), "body should show /fn/greet call URL");
    // Empty call log.
    assert!(body.contains("No calls"), "body should show empty-state; got: {body}");
}

#[tokio::test]
async fn detail_page_shows_call_log() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let (_, plaintext) = ApiKey::create(&app.pool, user.id, "k").await.unwrap();
    let code = "def handler(x: int) -> int:\n    return x * 2\n";
    let f = Function::create(
        &app.pool, user.id, "doubler", code, "stateless", None, false, true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    // Make a real API call to generate a function_calls row.
    let call_res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/fn/doubler")
                .header("authorization", format!("Bearer {plaintext}"))
                .header("content-type", "application/json")
                .body(Body::from("{\"x\": 21}".to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(call_res.status(), StatusCode::OK);

    let res = get_path(&app, &cookie, &format!("/functions/{}", f.id)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(
        !body.contains("No calls yet"),
        "call log should not be empty; got: {body}"
    );
    // Status column should show a 200.
    assert!(body.contains("200"), "detail page should show call status; got: {body}");
}

#[tokio::test]
async fn save_code_via_detail_form() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let code = "def handler(): return 1\n";
    let f = Function::create(
        &app.pool, user.id, "edit", code, "stateless", None, false, true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    // URL-encoded: `code` = "def handler():\n    return 42\n"
    let form = "code=def+handler%28%29%3A%0A++++return+42%0A";
    let res = post_form(&app, &cookie, &format!("/functions/{}", f.id), form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        res.headers().get("location").unwrap().to_str().unwrap(),
        &format!("/functions/{}", f.id)
    );

    // Detail page now shows the new code.
    let res = get_path(&app, &cookie, &format!("/functions/{}", f.id)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("return 42"), "new code should appear; got: {body}");

    // And the DB reflects the update (plus is_public/truncate_logs were toggled off,
    // because the form didn't include those checkboxes).
    let fetched = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap();
    assert!(fetched.code.contains("return 42"));
    assert!(!fetched.is_public);
    assert!(!fetched.truncate_logs);
}

#[tokio::test]
async fn rotate_public_url_changes_url() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let code = "def handler(): return 1\n";
    let f = Function::create(
        &app.pool, user.id, "pubfn", code, "stateless", None, true, true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    // Capture the initial public URL from the detail page.
    let res = get_path(&app, &cookie, &format!("/functions/{}", f.id)).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body_before = body_string(res).await;
    let initial_token = f.public_token.clone().expect("function starts public");
    assert!(
        body_before.contains(&format!("/fn/p/{initial_token}")),
        "initial detail page should show the starting token"
    );

    // Rotate.
    let rotate = post_form(
        &app,
        &cookie,
        &format!("/functions/{}/rotate", f.id),
        "",
    )
    .await;
    assert_eq!(rotate.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        rotate.headers().get("location").unwrap().to_str().unwrap(),
        &format!("/functions/{}", f.id)
    );

    // Detail page now shows a new token, different from the original.
    let res = get_path(&app, &cookie, &format!("/functions/{}", f.id)).await;
    let body_after = body_string(res).await;
    let new_token = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap()
        .public_token
        .unwrap();
    assert_ne!(initial_token, new_token);
    assert!(body_after.contains(&format!("/fn/p/{new_token}")));
    assert!(!body_after.contains(&format!("/fn/p/{initial_token}")));
}

#[tokio::test]
async fn invalid_code_save_shows_error() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let code = "def handler(): return 1\n";
    let f = Function::create(
        &app.pool, user.id, "badedit", code, "stateless", None, false, true,
    )
    .await
    .unwrap();
    let cookie = app.sign_in(user.id).await;

    // No `def handler(...)` → parse_handler rejects.
    let form = "code=def+other%28%29%3A+pass%0A";
    let res = post_form(&app, &cookie, &format!("/functions/{}", f.id), form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with(&format!("/functions/{}?error=", f.id)),
        "expected redirect to detail with ?error=..., got {location}"
    );

    // Detail page renders the error banner.
    let res = get_path(&app, &cookie, &location).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(
        body.contains("invalid code"),
        "detail page should surface the error; got: {body}"
    );

    // And the DB was NOT updated.
    let fetched = Function::find_for_user(&app.pool, user.id, &f.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.code, code, "code should be unchanged on validation failure");
}
