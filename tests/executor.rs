mod common;
use monty_server::envs::executor::{EnvExecutor, MontyExecutor};
use monty_server::envs::locks::LockRegistry;
use monty_server::envs::storage::EnvStorage;

async fn setup() -> (tempfile::TempDir, EnvStorage, MontyExecutor) {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let locks = LockRegistry::new();
    let exec = MontyExecutor::new(storage.clone(), locks, common::sandbox().await);
    (tmp, storage, exec)
}

#[tokio::test]
async fn stateful_execution_preserves_variables_across_calls() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest1").await.unwrap();

    let out1 = exec.execute("envtest1", "x = 40 + 2").await.unwrap();
    assert!(out1.error.is_none(), "first call errored: {:?}", out1.error);

    let out2 = exec.execute("envtest1", "print(x)").await.unwrap();
    assert!(out2.error.is_none(), "second call errored: {:?}", out2.error);
    assert_eq!(out2.stdout.trim(), "42");
}

#[tokio::test]
async fn one_off_does_not_persist_to_any_env() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest2").await.unwrap();

    exec.execute_one_off("x = 99").await.unwrap();
    // Referencing `x` in env should NameError because one-off wrote nothing.
    let out = exec.execute("envtest2", "x").await.unwrap();
    assert!(
        out.error.is_some(),
        "expected NameError, got: {out:?}"
    );
}

#[tokio::test]
async fn sandbox_can_write_its_files_dir() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest3").await.unwrap();

    let out = exec
        .execute(
            "envtest3",
            r#"
from pathlib import Path
Path("/mnt/hello.txt").write_text("hi")
"#,
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "write errored: {:?}", out.error);

    let contents =
        std::fs::read_to_string(storage.files_dir("envtest3").unwrap().join("hello.txt"))
            .unwrap();
    assert_eq!(contents, "hi");
}

#[tokio::test]
async fn sandbox_cannot_escape_mount_via_dotdot() {
    let (tmp, storage, exec) = setup().await;
    storage.create("envtest4").await.unwrap();

    // Place a secret outside the mount root.
    std::fs::write(tmp.path().join("secret.txt"), "TOP-SECRET").unwrap();

    let out = exec
        .execute(
            "envtest4",
            r#"
from pathlib import Path
Path("/mnt/../secret.txt").read_text()
"#,
        )
        .await
        .unwrap();
    assert!(
        out.error.is_some(),
        "path traversal should have errored, got stdout={:?}",
        out.stdout
    );
}

#[tokio::test]
async fn result_is_python_repr_and_none_is_hidden() {
    let (_tmp, _storage, exec) = setup().await;
    // (monty-types 1.5.0 drops the trailing comma of a 1-tuple's repr, so
    // stick to a 2-tuple here.)
    let out = exec.execute_one_off("[1, 'a', (2, 3)]").await.unwrap();
    assert_eq!(out.result.as_deref(), Some("[1, 'a', (2, 3)]"));
    let out = exec.execute_one_off("x = 1").await.unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.result, None);
}

#[tokio::test]
async fn failed_call_keeps_prior_state() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest5").await.unwrap();
    exec.execute("envtest5", "x = 1").await.unwrap();
    let out = exec.execute("envtest5", "x = 2\nraise ValueError('boom')").await.unwrap();
    assert!(out.error.as_deref().unwrap().contains("ValueError: boom"), "{:?}", out.error);
    let out = exec.execute("envtest5", "x").await.unwrap();
    assert_eq!(out.result.as_deref(), Some("1"));
}

#[tokio::test]
async fn stderr_is_captured_separately() {
    let (_tmp, _storage, exec) = setup().await;
    let out = exec
        .execute_one_off("import sys\nprint('out')\nprint('err', file=sys.stderr)")
        .await
        .unwrap();
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

#[tokio::test]
async fn memory_limit_is_enforced_and_server_survives() {
    let (_tmp, _storage, exec) = setup().await;
    let out = exec.execute_one_off("x = 'a' * (512 * 1024 * 1024)").await.unwrap();
    assert!(out.error.as_deref().unwrap_or("").contains("MemoryError"), "{out:?}");
    // The pool replaces or reuses workers; the next call still works.
    let out = exec.execute_one_off("1 + 1").await.unwrap();
    assert_eq!(out.result.as_deref(), Some("2"));
}

#[tokio::test]
async fn time_limit_is_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let sandbox = monty_server::sandbox::Sandbox::new(monty_server::sandbox::SandboxConfig {
        worker_bin: monty_server::config::monty_worker_bin(),
        min_workers: 0,
        limits: monty_server::sandbox::ExecLimits {
            max_duration: std::time::Duration::from_millis(300),
            ..Default::default()
        },
        ..Default::default()
    })
    .await
    .unwrap();
    let exec = MontyExecutor::new(storage, LockRegistry::new(), std::sync::Arc::new(sandbox));
    let out = exec.execute_one_off("while True:\n    pass").await.unwrap();
    assert!(out.error.as_deref().unwrap_or("").contains("TimeoutError"), "{out:?}");
}

#[tokio::test]
async fn legacy_snapshot_is_archived_and_env_reset() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest6").await.unwrap();
    // Bytes in the pre-1.0 (headerless postcard) format.
    storage.write_snapshot("envtest6", &[1, 2, 3, 4]).await.unwrap();
    let out = exec.execute("envtest6", "y = 5\ny").await.unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.result.as_deref(), Some("5"));
    assert!(out.stderr.contains("older Monty version"), "{:?}", out.stderr);
    let env_dir = storage.files_dir("envtest6").unwrap().parent().unwrap().to_path_buf();
    assert_eq!(std::fs::read(env_dir.join("snapshot.bin.legacy")).unwrap(), vec![1, 2, 3, 4]);
    // State saved in the new format carries over.
    let out = exec.execute("envtest6", "y + 1").await.unwrap();
    assert_eq!(out.result.as_deref(), Some("6"));
    assert!(!out.stderr.contains("older Monty version"));
}

#[tokio::test]
async fn relative_paths_resolve_inside_mount() {
    let (_tmp, storage, exec) = setup().await;
    storage.create("envtest7").await.unwrap();
    let out = exec
        .execute("envtest7", "from pathlib import Path\nPath('rel.txt').write_text('ok')")
        .await
        .unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    let p = storage.files_dir("envtest7").unwrap().join("rel.txt");
    assert_eq!(std::fs::read_to_string(p).unwrap(), "ok");
}
