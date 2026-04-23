use monty_server::envs::storage::EnvStorage;

#[tokio::test]
async fn create_env_dir_creates_files_dir_and_empty_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    let id = "abcdefgh".to_string();

    storage.create(&id).await.unwrap();

    assert!(tmp.path().join("envs").join(&id).join("files").is_dir());
    assert!(tmp.path().join("envs").join(&id).join("snapshot.bin").is_file());
}

#[tokio::test]
async fn delete_env_removes_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    storage.create("abcdefgh").await.unwrap();
    storage.delete("abcdefgh").await.unwrap();
    assert!(!tmp.path().join("envs").join("abcdefgh").exists());
}

#[tokio::test]
async fn write_snapshot_is_atomic_and_readable() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    storage.create("abcdefgh").await.unwrap();
    storage.write_snapshot("abcdefgh", b"v1").await.unwrap();
    storage
        .write_snapshot("abcdefgh", b"v2_longer")
        .await
        .unwrap();
    assert_eq!(storage.read_snapshot("abcdefgh").await.unwrap(), b"v2_longer");
}

#[tokio::test]
async fn rejects_invalid_env_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    // traversal
    assert!(storage.create("../etc").await.is_err());
    // too short
    assert!(storage.create("abc").await.is_err());
    // uppercase
    assert!(storage.create("ABCDEFGH").await.is_err());
    // slashes
    assert!(storage.create("a/b/c/d").await.is_err());
}

#[tokio::test]
async fn files_dir_is_inside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = EnvStorage::new(tmp.path());
    storage.create("abcdefgh").await.unwrap();
    let files_dir = storage.files_dir("abcdefgh").unwrap();
    assert!(files_dir.starts_with(tmp.path()));
    assert!(files_dir.ends_with("files"));
}
