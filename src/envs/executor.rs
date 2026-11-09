use anyhow::{Context, Result};
use serde::Serialize;
use std::sync::Arc;

use crate::envs::locks::LockRegistry;
use crate::envs::storage::EnvStorage;
use crate::sandbox::{Restored, RunOutcome, Sandbox, repl_echo};

pub use crate::sandbox::ExecLimits;

/// Shown on the first call into an env whose saved state predates monty 1.0.
const LEGACY_SNAPSHOT_NOTICE: &str = "[monty-server] This env's saved variables were created by an \
older Monty version and could not be loaded, so it has been reset. Files in /mnt are unaffected.\n";

#[derive(Debug, Clone, Serialize)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// `repr()` of the snippet's final expression; `None` when it is `None`.
    pub result: Option<String>,
    pub error: Option<String>,
}

#[axum::async_trait]
pub trait EnvExecutor: Send + Sync + 'static {
    async fn execute(&self, env_id: &str, code: &str) -> Result<ExecOutput>;
    async fn execute_one_off(&self, code: &str) -> Result<ExecOutput>;
}

/// Monty-backed executor. Each call to `execute(env_id, code)`:
///   1. Takes the env lock.
///   2. Checks out a worker and restores the env snapshot into it (if any).
///   3. Mounts `<data>/envs/<id>/files/` read-write at `/mnt`.
///   4. Feeds the snippet; file I/O is serviced by the server against the mount.
///   5. On success, dumps the session and writes the snapshot atomically.
///   6. On error, leaves the prior snapshot in place.
#[derive(Clone)]
pub struct MontyExecutor {
    storage: EnvStorage,
    locks: LockRegistry,
    sandbox: Arc<Sandbox>,
}

impl MontyExecutor {
    pub fn new(storage: EnvStorage, locks: LockRegistry, sandbox: Arc<Sandbox>) -> Self {
        Self {
            storage,
            locks,
            sandbox,
        }
    }

    pub fn locks(&self) -> &LockRegistry {
        &self.locks
    }
}

#[axum::async_trait]
impl EnvExecutor for MontyExecutor {
    async fn execute(&self, env_id: &str, code: &str) -> Result<ExecOutput> {
        let _guard = self.locks.acquire(env_id).await;

        let snapshot = self
            .storage
            .read_snapshot(env_id)
            .await
            .context("read env snapshot")?;
        let files_dir = self.storage.files_dir(env_id)?;
        let mount = || self.sandbox.mount(&files_dir, true);

        let mut session = self.sandbox.session("main.py").await?;
        let mut notice = "";
        match session.restore(snapshot, vec![mount()?]).await? {
            Restored::Empty | Restored::Loaded => {}
            Restored::Legacy => {
                tracing::warn!(env_id, "archiving pre-1.0 monty snapshot");
                self.storage.archive_snapshot(env_id).await?;
                notice = LEGACY_SNAPSHOT_NOTICE;
            }
            Restored::Failed(error) => {
                return Ok(ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    result: None,
                    error: Some(error),
                });
            }
        }

        let outcome = session.run(code, Vec::new(), vec![mount()?]).await?;
        let (result, error) = match outcome {
            RunOutcome::Complete(value) => {
                let bytes = session.dump().await?;
                self.storage
                    .write_snapshot(env_id, &bytes)
                    .await
                    .context("write updated snapshot")?;
                (repl_echo(&value), None)
            }
            RunOutcome::Failed(error) => (None, Some(error)),
        };
        let output = session.take_output();
        session.finish().await;
        Ok(ExecOutput {
            stdout: output.stdout,
            stderr: format!("{notice}{}", output.stderr),
            result,
            error,
        })
    }

    async fn execute_one_off(&self, code: &str) -> Result<ExecOutput> {
        let mut session = self.sandbox.session("main.py").await?;
        // No mounts — one-off calls cannot touch the filesystem.
        let outcome = session.run(code, Vec::new(), Vec::new()).await?;
        let output = session.take_output();
        session.finish().await;
        let (result, error) = match outcome {
            RunOutcome::Complete(value) => (repl_echo(&value), None),
            RunOutcome::Failed(error) => (None, Some(error)),
        };
        Ok(ExecOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            result,
            error,
        })
    }
}
