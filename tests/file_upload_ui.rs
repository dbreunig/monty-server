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
async fn ui_upload_adds_file_and_returns_updated_list() {
    let app = TestApp::new().await;
    let user = User::upsert(&app.pool, 1, "u", None).await.unwrap();
    let cookie = app.sign_in(user.id).await;
    let env = Env::create(&app.pool, user.id, "scratch").await.unwrap();
    EnvStorage::new(app.tmp.path()).create(&env.id).await.unwrap();

    let boundary = "X-BOUNDARY";
    let body = multipart_body(boundary, "data.csv", b"a,b\n1,2\n");
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/envs/{}/files", env.id))
                .header("cookie", &cookie)
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
    let fragment = body_string(res).await;
    assert!(fragment.contains("data.csv"), "list fragment must render file: {fragment}");

    assert!(
        app.tmp
            .path()
            .join("envs")
            .join(&env.id)
            .join("files")
            .join("data.csv")
            .exists()
    );
}
