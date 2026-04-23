use anyhow::{Context, Result};
use monty::fs::{MountMode, MountTable};
use monty::{
    ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRepl, PrintStream,
    PrintWriter, ReplProgress, ReplStartError, ResourceLimits,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::envs::locks::LockRegistry;
use crate::envs::storage::EnvStorage;

const SANDBOX_MOUNT: &str = "/mnt";

/// Per-call resource limits for Monty execution.
#[derive(Clone, Debug)]
pub struct ExecLimits {
    pub max_memory_bytes: u64,
    pub max_duration: Duration,
    pub max_allocations: u64,
    pub max_write_bytes: Option<u64>,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 128 * 1024 * 1024,
            max_duration: Duration::from_secs(30),
            max_allocations: 10_000_000,
            max_write_bytes: Some(50 * 1024 * 1024),
        }
    }
}

impl ExecLimits {
    fn tracker(&self) -> LimitedTracker {
        LimitedTracker::new(
            ResourceLimits::new()
                .max_memory(self.max_memory_bytes as usize)
                .max_duration(self.max_duration)
                .max_allocations(self.max_allocations as usize),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
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
///   2. Loads the `MontyRepl` snapshot from disk (or builds a fresh one).
///   3. Mounts `<data>/envs/<id>/files/` at `/mnt` inside the sandbox.
///   4. Feeds the snippet, driving any OS calls through the mount table.
///   5. Dumps the updated REPL back to disk atomically on success.
///   6. Preserves the prior snapshot on error.
#[derive(Clone)]
pub struct MontyExecutor {
    storage: EnvStorage,
    locks: LockRegistry,
    limits: ExecLimits,
}

impl MontyExecutor {
    pub fn new(storage: EnvStorage, locks: LockRegistry) -> Self {
        Self {
            storage,
            locks,
            limits: ExecLimits::default(),
        }
    }

    #[allow(dead_code)]
    pub fn with_limits(mut self, limits: ExecLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn locks(&self) -> &LockRegistry {
        &self.locks
    }
}

#[axum::async_trait]
impl EnvExecutor for MontyExecutor {
    async fn execute(&self, env_id: &str, code: &str) -> Result<ExecOutput> {
        let _guard = self.locks.acquire(env_id).await;

        let bytes = self
            .storage
            .read_snapshot(env_id)
            .await
            .context("read env snapshot")?;
        let files_dir = self.storage.files_dir(env_id)?;
        let limits = self.limits.clone();
        let code = code.to_string();

        let job = tokio::task::spawn_blocking(move || {
            let repl = if bytes.is_empty() {
                MontyRepl::new("<env>", limits.tracker())
            } else {
                let mut loaded: MontyRepl<LimitedTracker> =
                    MontyRepl::load(&bytes).context("load repl snapshot")?;
                // Reset per-call budget (time, allocations) while keeping heap state.
                *loaded.tracker_mut() = limits.tracker();
                loaded
            };
            let (outcome, new_repl) = feed_in_sandbox(repl, &code, &files_dir, &limits)?;
            let new_bytes = new_repl.dump().context("dump repl snapshot")?;
            Ok::<_, anyhow::Error>((outcome, new_bytes))
        })
        .await
        .context("spawn_blocking panicked")??;

        let (outcome, new_bytes) = job;
        if outcome.error.is_none() {
            self.storage
                .write_snapshot(env_id, &new_bytes)
                .await
                .context("write updated snapshot")?;
        }
        Ok(outcome)
    }

    async fn execute_one_off(&self, code: &str) -> Result<ExecOutput> {
        let limits = self.limits.clone();
        let code = code.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            let repl = MontyRepl::new("<one-off>", limits.tracker());
            // No mounts — one-off calls cannot touch the filesystem.
            let (outcome, _) = feed_in_sandbox(repl, &code, &PathBuf::new(), &limits)?;
            Ok::<_, anyhow::Error>(outcome)
        })
        .await
        .context("spawn_blocking panicked")??;
        Ok(outcome)
    }
}

fn feed_in_sandbox(
    repl: MontyRepl<LimitedTracker>,
    code: &str,
    files_dir: &PathBuf,
    limits: &ExecLimits,
) -> Result<(ExecOutput, MontyRepl<LimitedTracker>)> {
    let mut mount_table = MountTable::new();
    if !files_dir.as_os_str().is_empty() {
        mount_table
            .mount(
                SANDBOX_MOUNT,
                files_dir,
                MountMode::ReadWrite,
                limits.max_write_bytes,
            )
            .context("mount env files dir")?;
    }
    let mut streams: Vec<(PrintStream, String)> = Vec::new();
    let writer = PrintWriter::CollectStreams(&mut streams);
    let progress = match repl.feed_start(code, Vec::new(), writer) {
        Ok(p) => p,
        Err(err) => {
            let ReplStartError { repl, error } = *err;
            let (stdout, stderr) = split_streams(&streams);
            let out = ExecOutput {
                stdout,
                stderr,
                result: None,
                error: Some(format_exception(&error)),
            };
            return Ok((out, repl));
        }
    };
    drive(progress, &mut mount_table, &mut streams).map(|(out, repl)| {
        let (stdout, stderr) = split_streams(&streams);
        (
            ExecOutput {
                stdout,
                stderr,
                ..out
            },
            repl,
        )
    })
}

fn drive(
    mut progress: ReplProgress<LimitedTracker>,
    mount_table: &mut MountTable,
    streams: &mut Vec<(PrintStream, String)>,
) -> Result<(ExecOutput, MontyRepl<LimitedTracker>)> {
    loop {
        progress = match progress {
            ReplProgress::Complete { repl, value } => {
                let out = ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    result: Some(format_object(&value)),
                    error: None,
                };
                return Ok((out, repl));
            }
            ReplProgress::OsCall(call) => {
                let writer = PrintWriter::CollectStreams(streams);
                let result = match mount_table.handle_os_call(call.function, &call.args, &call.kwargs) {
                    Some(Ok(obj)) => ExtFunctionResult::Return(obj),
                    Some(Err(err)) => ExtFunctionResult::Error(err.into_exception()),
                    None => ExtFunctionResult::NotFound(format!("{:?}", call.function)),
                };
                match call.resume(result, writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl, error } = *err;
                        let out = ExecOutput {
                            stdout: String::new(),
                            stderr: String::new(),
                            result: None,
                            error: Some(format_exception(&error)),
                        };
                        return Ok((out, repl));
                    }
                }
            }
            ReplProgress::FunctionCall(call) => {
                // We don't support external functions yet — reply with NotFound.
                let writer = PrintWriter::CollectStreams(streams);
                let name = call.function_name.clone();
                match call.resume(ExtFunctionResult::NotFound(name), writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl, error } = *err;
                        let out = ExecOutput {
                            stdout: String::new(),
                            stderr: String::new(),
                            result: None,
                            error: Some(format_exception(&error)),
                        };
                        return Ok((out, repl));
                    }
                }
            }
            ReplProgress::NameLookup(lookup) => {
                let repl = lookup.into_repl();
                let out = ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    result: None,
                    error: Some("name lookup unsupported".into()),
                };
                return Ok((out, repl));
            }
            ReplProgress::ResolveFutures(state) => {
                let repl = state.into_repl();
                let out = ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    result: None,
                    error: Some("async futures unsupported".into()),
                };
                return Ok((out, repl));
            }
        };
    }
}

fn split_streams(streams: &[(PrintStream, String)]) -> (String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    for (stream, text) in streams {
        match stream {
            PrintStream::Stdout => stdout.push_str(text),
            PrintStream::Stderr => stderr.push_str(text),
        }
    }
    (stdout, stderr)
}

fn format_object(obj: &MontyObject) -> String {
    format!("{obj:?}")
}

fn format_exception(exc: &MontyException) -> String {
    format!("{exc}")
}

// The `Deserialize` derive on SnapshotEnvelope is reserved for future versioning.
#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct SnapshotEnvelope {
    version: u32,
}
