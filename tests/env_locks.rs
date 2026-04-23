use monty_server::envs::locks::LockRegistry;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering::SeqCst};
use std::time::Duration;

#[tokio::test]
async fn concurrent_ops_on_same_env_are_serialized() {
    let locks = LockRegistry::new();
    let counter = Arc::new(AtomicI32::new(0));
    let max_seen = Arc::new(AtomicI32::new(0));

    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let locks = locks.clone();
            let counter = counter.clone();
            let max_seen = max_seen.clone();
            tokio::spawn(async move {
                let _guard = locks.acquire("env1").await;
                let now = counter.fetch_add(1, SeqCst) + 1;
                max_seen.fetch_max(now, SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                counter.fetch_sub(1, SeqCst);
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(
        max_seen.load(SeqCst),
        1,
        "never more than 1 holder at a time"
    );
}

#[tokio::test]
async fn different_envs_do_not_block() {
    let locks = LockRegistry::new();
    let concurrent = Arc::new(AtomicI32::new(0));
    let max_concurrent = Arc::new(AtomicI32::new(0));

    let tasks: Vec<_> = (0..10)
        .map(|i| {
            let locks = locks.clone();
            let concurrent = concurrent.clone();
            let max_concurrent = max_concurrent.clone();
            tokio::spawn(async move {
                let env_id = format!("env-{i:08}");
                let _guard = locks.acquire(&env_id).await;
                let now = concurrent.fetch_add(1, SeqCst) + 1;
                max_concurrent.fetch_max(now, SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                concurrent.fetch_sub(1, SeqCst);
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    assert!(
        max_concurrent.load(SeqCst) > 1,
        "different envs should run in parallel"
    );
}

#[tokio::test]
async fn release_evicts_when_no_holders() {
    let locks = LockRegistry::new();
    {
        let _g = locks.acquire("abcdef12").await;
        assert_eq!(locks.len(), 1);
    }
    // Allow drop to run; Drop is synchronous so this should be instant.
    assert_eq!(locks.len(), 0, "entry evicted when last holder drops");
}
