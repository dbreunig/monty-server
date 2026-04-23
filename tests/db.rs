#[tokio::test]
async fn migrations_run_and_users_table_exists() {
    let pool = monty_server::db::connect(":memory:").await.unwrap();
    monty_server::db::migrate(&pool).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn api_keys_and_envs_tables_exist() {
    let pool = monty_server::db::connect(":memory:").await.unwrap();
    monty_server::db::migrate(&pool).await.unwrap();
    let _: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM api_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    let _: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM envs")
        .fetch_one(&pool)
        .await
        .unwrap();
}
