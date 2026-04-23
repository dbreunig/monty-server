use monty_server::envs::executor::{EnvExecutor, MontyExecutor};
use monty_server::envs::locks::LockRegistry;
use monty_server::envs::storage::EnvStorage;

async fn setup() -> (tempfile::TempDir, EnvStorage, MontyExecutor) {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let locks = LockRegistry::new();
    let exec = MontyExecutor::new(storage.clone(), locks);
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
