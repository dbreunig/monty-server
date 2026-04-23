use monty_server::{db, models::User};
use sqlx::SqlitePool;

async fn test_pool() -> SqlitePool {
    let pool = db::connect(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn upsert_user_creates_and_then_updates() {
    let pool = test_pool().await;
    let u1 = User::upsert(&pool, 42, "octocat", Some("o@example.com"))
        .await
        .unwrap();
    let u2 = User::upsert(&pool, 42, "octocat-renamed", None)
        .await
        .unwrap();
    assert_eq!(u1.id, u2.id);
    assert_eq!(u2.github_login, "octocat-renamed");
    assert_eq!(u2.email, None);
}

#[tokio::test]
async fn find_by_id_returns_user() {
    let pool = test_pool().await;
    let u = User::upsert(&pool, 7, "alice", None).await.unwrap();
    let found = User::find_by_id(&pool, u.id).await.unwrap().unwrap();
    assert_eq!(found.github_id, 7);
    assert_eq!(found.github_login, "alice");
}

#[tokio::test]
async fn find_by_id_missing_returns_none() {
    let pool = test_pool().await;
    assert!(User::find_by_id(&pool, 999).await.unwrap().is_none());
}
