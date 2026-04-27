mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::models::{Function, FunctionCall, User};
use tower::ServiceExt;

/// End-to-end smoke test: create a public function via the web form, then
/// hit the public URL with no auth at all (simulating an external webhook
/// caller) and confirm the call succeeds and is logged.
#[tokio::test]
async fn public_webhook_flow_end_to_end() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;

    // 1) Create the function via the web form, is_public = true.
    //    code = "def handler(x: int) -> int:\n    return x + 1\n"
    let form = "slug=webhook&target=stateless&is_public=on\
&code=def+handler%28x%3A+int%29+-%3E+int%3A%0A++++return+x+%2B+1%0A";
    let create_res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/functions")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_res.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        create_res.headers().get("location").unwrap(),
        "/dashboard",
        "create should redirect to /dashboard"
    );

    // 2) Follow the redirect to confirm the function landed.
    let dash_res = app
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
    assert_eq!(dash_res.status(), StatusCode::OK);
    let dash_body = dash_res.into_body().collect().await.unwrap().to_bytes();
    let dash_html = String::from_utf8(dash_body.to_vec()).unwrap();
    assert!(
        dash_html.contains("webhook"),
        "dashboard should list the new function; got: {dash_html}"
    );

    // 3) Look up the function by slug; fetch its public_token via the model.
    let function = Function::find_by_slug_for_user(&app.pool, user.id, "webhook")
        .await
        .unwrap()
        .expect("function should exist after create");
    assert!(function.is_public, "function should have been created public");
    let public_token = function
        .public_token
        .clone()
        .expect("public function should have a minted public_token");

    // 4) Sanity: the detail page advertises the public URL.
    let detail_res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/functions/{}", function.id))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_res.status(), StatusCode::OK);
    let detail_body = detail_res.into_body().collect().await.unwrap().to_bytes();
    let detail_html = String::from_utf8(detail_body.to_vec()).unwrap();
    assert!(
        detail_html.contains(&format!("/fn/p/{public_token}")),
        "detail page should display the public URL; got: {detail_html}"
    );

    // 5) Hit the public URL with NO cookie and NO bearer token.
    let call_res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/fn/p/{public_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "x": 41 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        call_res.status(),
        StatusCode::OK,
        "unauthenticated public webhook call should succeed"
    );
    let call_body = call_res.into_body().collect().await.unwrap().to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&call_body)
        .expect("public webhook response should be JSON");
    assert_eq!(payload, serde_json::json!(42));

    // 6) The call should have been logged.
    let logs = FunctionCall::list_recent(&app.pool, &function.id, 10)
        .await
        .unwrap();
    assert!(
        !logs.is_empty(),
        "expected at least one function_calls row for {}",
        function.id
    );
    assert!(
        logs.iter().any(|row| row.status == 200),
        "expected a 200 row in call log; got: {:?}",
        logs.iter().map(|r| r.status).collect::<Vec<_>>()
    );
}
