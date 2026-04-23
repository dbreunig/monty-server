use monty_server::{
    db,
    models::{ApiKey, User},
};
use sqlx::SqlitePool;

async fn test_pool() -> SqlitePool {
    let pool = db::connect(":memory:").await.unwrap();
    db::migrate(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn create_returns_plaintext_once_and_hash_verifies() {
    let pool = test_pool().await;
    let user = User::upsert_github(&pool, 1, "u", None).await.unwrap();

    let (_key, plaintext) = ApiKey::create(&pool, user.id, "laptop").await.unwrap();
    assert!(plaintext.starts_with("mk_"));
    assert_eq!(plaintext.len(), 3 + 32);

    let found = ApiKey::verify(&pool, &plaintext).await.unwrap().unwrap();
    assert_eq!(found.user_id, user.id);

    assert!(
        ApiKey::verify(&pool, "mk_wrongwrongwrongwrongwrongwrong")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn list_for_user_returns_only_own_keys() {
    let pool = test_pool().await;
    let alice = User::upsert_github(&pool, 1, "alice", None).await.unwrap();
    let bob = User::upsert_github(&pool, 2, "bob", None).await.unwrap();
    ApiKey::create(&pool, alice.id, "a1").await.unwrap();
    ApiKey::create(&pool, alice.id, "a2").await.unwrap();
    ApiKey::create(&pool, bob.id, "b1").await.unwrap();

    let alice_keys = ApiKey::list_for_user(&pool, alice.id).await.unwrap();
    let bob_keys = ApiKey::list_for_user(&pool, bob.id).await.unwrap();
    assert_eq!(alice_keys.len(), 2);
    assert_eq!(bob_keys.len(), 1);
}

#[tokio::test]
async fn delete_removes_key_and_enforces_ownership() {
    let pool = test_pool().await;
    let alice = User::upsert_github(&pool, 1, "alice", None).await.unwrap();
    let bob = User::upsert_github(&pool, 2, "bob", None).await.unwrap();
    let (alice_key, _) = ApiKey::create(&pool, alice.id, "a1").await.unwrap();

    // Bob cannot delete Alice's key
    assert!(!ApiKey::delete(&pool, bob.id, alice_key.id).await.unwrap());
    // Alice can
    assert!(ApiKey::delete(&pool, alice.id, alice_key.id).await.unwrap());
    assert_eq!(
        ApiKey::list_for_user(&pool, alice.id).await.unwrap().len(),
        0
    );
}
