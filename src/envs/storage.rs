use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Filesystem layout:
///   `<root>/envs/<id>/files/`       — sandbox-mounted user files
///   `<root>/envs/<id>/snapshot.bin` — monty snapshot bytes (empty on create)
#[derive(Clone, Debug)]
pub struct EnvStorage {
    root: PathBuf,
}

impl EnvStorage {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.to_path_buf(),
        }
    }

    pub async fn create(&self, id: &str) -> Result<()> {
        let env_dir = self.env_dir(id)?;
        fs::create_dir_all(env_dir.join("files"))
            .await
            .context("create files dir")?;
        let snapshot = env_dir.join("snapshot.bin");
        if !snapshot.exists() {
            fs::File::create(&snapshot)
                .await
                .context("create empty snapshot")?;
        }
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let env_dir = self.env_dir(id)?;
        if env_dir.exists() {
            fs::remove_dir_all(&env_dir)
                .await
                .context("remove env dir")?;
        }
        Ok(())
    }

    pub async fn read_snapshot(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.env_dir(id)?.join("snapshot.bin");
        fs::read(&path).await.context("read snapshot")
    }

    pub async fn write_snapshot(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let env_dir = self.env_dir(id)?;
        let target = env_dir.join("snapshot.bin");
        let tmp = env_dir.join("snapshot.bin.tmp");
        {
            let mut f = fs::File::create(&tmp)
                .await
                .context("open snapshot tmp")?;
            f.write_all(bytes).await.context("write snapshot bytes")?;
            f.sync_all().await.context("fsync snapshot")?;
        }
        fs::rename(&tmp, &target)
            .await
            .context("rename snapshot into place")?;
        Ok(())
    }

    pub fn files_dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.env_dir(id)?.join("files"))
    }

    fn env_dir(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join("envs").join(id))
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() < 8 {
        bail!("env id too short: {id}");
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("env id contains invalid characters: {id}");
    }
    Ok(())
}
