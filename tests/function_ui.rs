mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{Env, Function, User};
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
    assert!(body.contains("name=\"mode\""));
    assert!(body.contains("name=\"code\""));
}

#[tokio::test]
async fn create_function_via_form_redirects_to_dashboard() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

    // URL-encoded form body. `code` = "def handler():\n    return 1\n"
    let form = "slug=webhook&mode=stateless&code=def+handler%28%29%3A%0A++++return+1%0A";
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
async fn create_bound_without_env_renders_error() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

    let form = "slug=needsenv&mode=bound&env_id=&code=def+handler%28%29%3A+return+1";
    let res = post_form(&app, &cookie, "/functions", form).await;
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

    // Follow the redirect and confirm the error message surfaces in the HTML.
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
        body.contains("env_id is required"),
        "dashboard should surface the validation error; body: {body}"
    );

    // And no function was created.
    let list = Function::list_for_user(&app.pool, user.id).await.unwrap();
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
        "slug=boundfn&mode=bound&env_id={}&code=def+handler%28%29%3A+return+1",
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
