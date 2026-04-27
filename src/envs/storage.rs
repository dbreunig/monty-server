use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

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

    pub async fn list_files(&self, id: &str) -> Result<Vec<FileEntry>> {
        let dir = self.files_dir(id)?;
        let mut out = Vec::new();
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context("read_dir"),
        };
        while let Some(entry) = rd.next_entry().await.context("next_entry")? {
            let meta = entry.metadata().await.context("metadata")?;
            if meta.is_file() {
                out.push(FileEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    size: meta.len(),
                    is_dir: false,
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Lists the immediate contents of `<files_dir>/<subpath>`. Directories
    /// come first, sorted alphabetically, then files. Returns an empty Vec if
    /// the target dir doesn't exist.
    pub async fn list_dir(&self, id: &str, subpath: &str) -> Result<Vec<FileEntry>> {
        validate_subpath(subpath)?;
        let dir = self.files_dir(id)?.join(subpath);
        let mut out = Vec::new();
        let mut rd = match fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context("read_dir"),
        };
        while let Some(entry) = rd.next_entry().await.context("next_entry")? {
            let meta = entry.metadata().await.context("metadata")?;
            let is_dir = meta.is_dir();
            if !is_dir && !meta.is_file() {
                continue;
            }
            out.push(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                size: if is_dir { 0 } else { meta.len() },
                is_dir,
            });
        }
        out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        Ok(out)
    }

    /// Returns true if `<files_dir>/<subpath>` exists and is a directory.
    pub async fn dir_exists(&self, id: &str, subpath: &str) -> Result<bool> {
        validate_subpath(subpath)?;
        let path = self.files_dir(id)?.join(subpath);
        match fs::metadata(&path).await {
            Ok(meta) => Ok(meta.is_dir()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("dir metadata"),
        }
    }

    pub async fn write_file(&self, id: &str, name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.resolve_file(id, name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.context("create parent dir")?;
        }
        fs::write(&path, bytes).await.context("write file")?;
        Ok(())
    }

    pub async fn read_file(&self, id: &str, name: &str) -> Result<Vec<u8>> {
        let path = self.resolve_file(id, name)?;
        fs::read(&path).await.context("read file")
    }

    pub async fn delete_file(&self, id: &str, name: &str) -> Result<bool> {
        let path = self.resolve_file(id, name)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("remove file"),
        }
    }

    /// Canonicalizes `files_dir(id) / name` and verifies the result stays
    /// inside the env's files root. This is the outside-of-Monty security
    /// boundary for filename inputs.
    fn resolve_file(&self, id: &str, name: &str) -> Result<PathBuf> {
        validate_filename(name)?;
        let files_dir = self.files_dir(id)?;
        // Ensure the files dir exists before canonicalizing.
        std::fs::create_dir_all(&files_dir).context("ensure files dir")?;
        let root_canon = std::fs::canonicalize(&files_dir).context("canonicalize files dir")?;
        let target = files_dir.join(name);
        // Canonicalize the *parent* so missing final component is allowed.
        let parent = target.parent().unwrap_or(&files_dir);
        std::fs::create_dir_all(parent).context("ensure parent dir")?;
        let parent_canon = std::fs::canonicalize(parent).context("canonicalize parent dir")?;
        let final_path = parent_canon.join(target.file_name().unwrap_or_default());
        if !final_path.starts_with(&root_canon) {
            bail!("path escapes env root: {name}");
        }
        Ok(final_path)
    }

    fn env_dir(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join("envs").join(id))
    }
}

/// Validates a path relative to an env's files dir. Empty string means "root".
/// Same charset as filenames; `/` separates segments; no `.`, `..`, leading `/`.
fn validate_subpath(path: &str) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') {
        bail!("path must be relative: {path}");
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            bail!("invalid path segment in: {path}");
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            bail!("path contains invalid characters: {path}");
        }
    }
    Ok(())
}

fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty filename");
    }
    if name.contains('\0') {
        bail!("filename contains null byte");
    }
    // Reject absolute paths and any `..` segments.
    for segment in name.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            bail!("invalid filename segment: {name}");
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            bail!("filename contains invalid characters: {name}");
        }
    }
    Ok(())
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
