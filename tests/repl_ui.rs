mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use http_body_util::BodyExt;
use monty_server::envs::storage::EnvStorage;
use monty_server::models::{Env, User};
use tower::ServiceExt;

async fn body_string(res: axum::response::Response) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn post_repl(app: &TestApp, env_id: &str, cookie: &str, code: &str) -> String {
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/envs/{env_id}/repl"))
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "code={}",
                    urlencoded(code)
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    body_string(res).await
}

fn urlencoded(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}

#[tokio::test]
async fn repl_fragments_persist_state_across_posts() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let env = Env::create(&app.pool, user.id, "scratch").await.unwrap();
    EnvStorage::new(app.tmp.path()).create(&env.id).await.unwrap();

    let _ = post_repl(&app, &env.id, &cookie, "x = 7").await;
    let frag = post_repl(&app, &env.id, &cookie, "print(x * 6)").await;
    assert!(
        frag.contains("42"),
        "second REPL call should print 42, got: {frag}"
    );
}

#[tokio::test]
async fn repl_history_renders_on_env_detail() {
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let env = Env::create(&app.pool, user.id, "scratch").await.unwrap();
    EnvStorage::new(app.tmp.path()).create(&env.id).await.unwrap();

    let _ = post_repl(&app, &env.id, &cookie, "x = 7").await;
    let _ = post_repl(&app, &env.id, &cookie, "print(x * 6)").await;

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
    assert!(body.contains("x = 7"), "history should include first call: {body}");
    assert!(body.contains("print(x * 6)"), "history should include second call");
    assert!(body.contains("42"), "history should include the printed output");
}

#[tokio::test]
async fn deleting_env_clears_repl_history() {
    use monty_server::models::ReplCall;
    let app = TestApp::new().await;
    let user = User::upsert_github(&app.pool, 1, "u", None).await.unwrap();
    User::approve(&app.pool, user.id).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let env = Env::create(&app.pool, user.id, "scratch").await.unwrap();
    EnvStorage::new(app.tmp.path()).create(&env.id).await.unwrap();
    let _ = post_repl(&app, &env.id, &cookie, "x = 1").await;
    assert_eq!(ReplCall::list_for_env(&app.pool, &env.id).await.unwrap().len(), 1);

    Env::delete_for_user(&app.pool, user.id, &env.id).await.unwrap();
    assert_eq!(ReplCall::list_for_env(&app.pool, &env.id).await.unwrap().len(), 0);
}

#[tokio::test]
async fn repl_for_other_users_env_is_404() {
    let app = TestApp::new().await;
    let alice = User::upsert_github(&app.pool, 1, "alice", None).await.unwrap();
    User::approve(&app.pool, alice.id).await.unwrap();
    let bob = User::upsert_github(&app.pool, 2, "bob", None).await.unwrap();
    User::approve(&app.pool, bob.id).await.unwrap();
    let env = Env::create(&app.pool, alice.id, "alice-env").await.unwrap();
    EnvStorage::new(app.tmp.path()).create(&env.id).await.unwrap();
    let cookie = app.sign_in(bob.id).await;

    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/envs/{}/repl", env.id))
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("code=1"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
