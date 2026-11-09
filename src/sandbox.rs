//! Shared Monty runtime: a pool of `monty subprocess` workers plus the loop
//! that drives one snippet through its suspensions.
//!
//! Untrusted code never runs in the server process. Each call checks out a
//! worker, optionally restores an env snapshot into it, feeds code, and either
//! returns the worker to the pool or discards it. A crash, a memory-limit kill
//! or a timeout takes down only that worker; the pool replaces it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use monty_pool::{
    Checkout, MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, ReplConfig, ResumeValue,
    TurnEvent, on_print_sync,
};
use monty_types::{
    ExcType, MontyException, MontyObject, NameLookupResult, NamedValues, PrintStream,
    ResourceLimits,
};

/// Virtual path env files are mounted at inside the sandbox.
pub const SANDBOX_MOUNT: &str = "/mnt";

/// Cap on captured stdout + stderr per call, so a print loop can't balloon
/// server memory. Output past this is dropped and a marker is appended.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Every dump written by monty >= 1.0 starts with this header. Snapshots from
/// the pre-1.0 git revision this server used to pin are raw postcard bytes
/// and cannot be restored.
const DUMP_MAGIC: &[u8] = b"MONTY\0";

/// Per-call resource limits.
#[derive(Clone, Debug)]
pub struct ExecLimits {
    /// Soft heap limit enforced inside the worker (`MemoryError` past it).
    pub max_memory_bytes: u64,
    /// Execution time per snippet, excluding time spent in host I/O.
    pub max_duration: Duration,
    /// Total `time.sleep` a snippet may request.
    pub max_total_sleep: Duration,
    /// Host round trips (file operations, sleeps) a snippet may make.
    pub max_suspensions: usize,
    /// Cap on bytes written through the `/mnt` mount per call.
    pub max_write_bytes: Option<u64>,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 128 * 1024 * 1024,
            max_duration: Duration::from_secs(30),
            max_total_sleep: Duration::from_secs(10),
            max_suspensions: 10_000,
            max_write_bytes: Some(50 * 1024 * 1024),
        }
    }
}

impl ExecLimits {
    fn resource_limits(&self) -> ResourceLimits {
        ResourceLimits::default()
            .max_memory(self.max_memory_bytes as usize)
            .max_feed_duration(self.max_duration)
            .max_total_sleep(self.max_total_sleep)
            .max_suspensions(self.max_suspensions)
    }

    /// Wall-clock backstop for a single worker round trip. The worker's own
    /// feed limit should fire first; this catches hangs it can't see.
    fn request_timeout(&self) -> Duration {
        self.max_duration + self.max_total_sleep + Duration::from_secs(5)
    }
}

/// How to spawn workers.
#[derive(Clone, Debug)]
pub struct SandboxConfig {
    /// Path to the `monty` binary from the `monty-runtime` crate (or a bare
    /// name to look up on `PATH`).
    pub worker_bin: PathBuf,
    /// Workers kept warm while idle.
    pub min_workers: usize,
    /// Upper bound on concurrent workers; further calls wait for one.
    pub max_workers: usize,
    /// How long a call waits for a free worker before failing.
    pub checkout_timeout: Duration,
    pub limits: ExecLimits,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            worker_bin: PathBuf::from("monty"),
            min_workers: 1,
            max_workers: 3,
            checkout_timeout: Duration::from_secs(30),
            limits: ExecLimits::default(),
        }
    }
}

/// Captured `print()` output for one call.
#[derive(Default)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    bytes: usize,
    truncated: bool,
}

impl Captured {
    fn push(&mut self, stream: PrintStream, text: &str) {
        if self.truncated {
            return;
        }
        let room = MAX_OUTPUT_BYTES.saturating_sub(self.bytes);
        let (text, cut) = if text.len() > room {
            let mut end = room;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            (&text[..end], true)
        } else {
            (text, false)
        };
        let target = match stream {
            PrintStream::Stdout => &mut self.stdout,
            PrintStream::Stderr => &mut self.stderr,
        };
        target.push_str(text);
        self.bytes += text.len();
        if cut {
            self.truncated = true;
            target.push_str("\n[output truncated]\n");
        }
    }
}

/// How a snippet ended, from the caller's point of view.
pub enum RunOutcome {
    /// The snippet ran to completion with this value.
    Complete(MontyObject),
    /// The snippet failed in a way the user should see: a Python exception,
    /// a resource limit, or a worker crash. The message is user-facing.
    Failed(String),
}

/// What became of a snapshot restore.
pub enum Restored {
    /// The env had no saved state.
    Empty,
    /// The snapshot loaded into the worker.
    Loaded,
    /// The snapshot predates monty 1.0 and can't be loaded.
    Legacy,
    /// The worker rejected the snapshot; the message is user-facing.
    Failed(String),
}

/// A pool of Monty workers shared by env and function executors.
pub struct Sandbox {
    pool: Pool,
    limits: ExecLimits,
}

impl Sandbox {
    pub async fn new(config: SandboxConfig) -> Result<Self> {
        let mut pool_config = PoolConfig::subprocess(&config.worker_bin);
        pool_config.min_processes = config.min_workers;
        pool_config.max_processes = config.max_workers.max(1);
        pool_config.request_timeout = Some(config.limits.request_timeout());
        pool_config.checkout_timeout = Some(config.checkout_timeout);
        let pool = Pool::new(pool_config).await.map_err(|e| {
            anyhow::anyhow!(
                "start monty worker pool using {}: {e}",
                config.worker_bin.display()
            )
        })?;
        Ok(Self {
            pool,
            limits: config.limits,
        })
    }

    pub fn limits(&self) -> &ExecLimits {
        &self.limits
    }

    /// Checks out a worker with a fresh session.
    pub async fn session(&self, script_name: &str) -> Result<Session> {
        let repl = ReplConfig {
            script_name: script_name.to_owned(),
            limits: Some(self.limits.resource_limits()),
            ..ReplConfig::default()
        };
        let checkout = self
            .pool
            .checkout(&repl)
            .await
            .map_err(|e| anyhow::anyhow!("check out monty worker: {e}"))?;
        Ok(Session {
            checkout: Some(checkout),
            reusable: true,
            output: Arc::new(Mutex::new(Captured::default())),
        })
    }

    /// Builds the `/mnt` mount for an env's files dir.
    pub fn mount(&self, files_dir: &Path, writable: bool) -> Result<MountSpec> {
        let mode = if writable {
            MountSpecMode::ReadWrite
        } else {
            MountSpecMode::ReadOnly
        };
        let mut spec = MountSpec::new(SANDBOX_MOUNT, files_dir, mode)
            .map_err(|e| anyhow::anyhow!("mount {}: {e}", files_dir.display()))?;
        spec.write_bytes_limit = self.limits.max_write_bytes;
        Ok(spec)
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// One checked-out worker. Call [`Session::finish`] when done; dropping a
/// session instead kills its worker.
pub struct Session {
    checkout: Option<Checkout>,
    /// Cleared after anything that leaves the worker in doubt (resource
    /// limits, crashes, protocol errors); `finish` then discards it.
    reusable: bool,
    output: Arc<Mutex<Captured>>,
}

impl Session {
    fn checkout(&mut self) -> Result<&mut Checkout> {
        self.checkout.as_mut().context("monty session already ended")
    }

    /// Loads a dump written by [`Session::dump`] into this session.
    pub async fn restore(&mut self, snapshot: Vec<u8>, mounts: Vec<MountSpec>) -> Result<Restored> {
        if snapshot.is_empty() {
            return Ok(Restored::Empty);
        }
        if !is_current_dump(&snapshot) {
            return Ok(Restored::Legacy);
        }
        let output = self.output.clone();
        let mut on_print = on_print_sync(move |s, t| output.lock().unwrap().push(s, t));
        let result = self.checkout()?.restore(snapshot, mounts, &mut on_print).await;
        match result {
            // Envs are only dumped while idle, so a restore should never
            // land mid-suspension.
            Ok((None, _)) => Ok(Restored::Loaded),
            Ok((Some(_), _)) => {
                self.discard();
                Ok(Restored::Failed("env snapshot was saved mid-execution".into()))
            }
            Err(PoolError::Runtime(exc)) => {
                // A failed restore discards the worker.
                self.discard();
                Ok(Restored::Failed(format!("could not restore env state: {exc}")))
            }
            Err(e) => {
                self.discard();
                Err(anyhow::anyhow!("restore env snapshot: {e}"))
            }
        }
    }

    /// Runs `code` to completion, answering suspensions: file operations go
    /// to `mounts`, sleeps are honoured, and everything else is unsupported.
    pub async fn run(
        &mut self,
        code: &str,
        inputs: Vec<(String, MontyObject)>,
        mounts: Vec<MountSpec>,
    ) -> Result<RunOutcome> {
        let output = self.output.clone();
        let mut on_print = on_print_sync(move |s, t| output.lock().unwrap().push(s, t));
        let checkout = self.checkout()?;
        let inputs: NamedValues = inputs.into();
        let mut step = checkout.feed(code, inputs, mounts, false, &mut on_print).await;
        loop {
            let event = match step {
                Ok(event) => event,
                Err(err) => return self.fail(err),
            };
            let checkout = self.checkout()?;
            step = match event {
                TurnEvent::Complete(value) => return Ok(RunOutcome::Complete(value)),
                TurnEvent::OsCall {
                    system_sleep: Some(delay),
                    ..
                } => {
                    tokio::time::sleep(delay).await;
                    checkout
                        .resume(ResumeValue::Return(MontyObject::none()), &mut on_print)
                        .await
                }
                TurnEvent::OsCall { .. } => match checkout.resume_from_mounts(&mut on_print).await {
                    Ok(Some(event)) => Ok(event),
                    Ok(None) => checkout.resume(ResumeValue::NotHandled, &mut on_print).await,
                    Err(err) => Err(err),
                },
                TurnEvent::FunctionCall { .. } => {
                    checkout.resume(ResumeValue::NotFound, &mut on_print).await
                }
                TurnEvent::NameLookup { .. } => {
                    checkout
                        .resume_name_lookup(NameLookupResult::Undefined, &mut on_print)
                        .await
                }
                TurnEvent::ResolveFutures { .. } => {
                    // We never hand out futures, so nothing can resolve them.
                    self.discard();
                    return Ok(RunOutcome::Failed(
                        "async external calls are not supported".into(),
                    ));
                }
            };
        }
    }

    /// Serializes the session so a later call can [`Session::restore`] it.
    pub async fn dump(&mut self) -> Result<Vec<u8>> {
        let result = self.checkout()?.dump().await;
        result.map_err(|e| {
            self.discard();
            anyhow::anyhow!("dump env state: {e}")
        })
    }

    /// Takes the output captured so far.
    pub fn take_output(&self) -> Captured {
        std::mem::take(&mut *self.output.lock().unwrap())
    }

    /// Returns the worker to the pool, or discards it if it's in doubt.
    pub async fn finish(mut self) {
        let Some(checkout) = self.checkout.take() else {
            return;
        };
        if self.reusable
            && let Err(e) = checkout.finish().await
        {
            tracing::warn!(error = %e, "monty worker failed to finish cleanly");
        }
        // Otherwise the checkout drops here, which kills the worker.
    }

    fn discard(&mut self) {
        self.reusable = false;
        self.checkout = None;
    }

    /// Maps a pool error to a user-facing failure, or to a server error for
    /// problems that aren't the snippet's fault.
    fn fail(&mut self, err: PoolError) -> Result<RunOutcome> {
        let message = match err {
            PoolError::Runtime(exc) => {
                if is_resource_error(&exc) {
                    // The worker survives a soft limit but its heap is
                    // suspect; don't hand it to the next caller.
                    self.reusable = false;
                }
                return Ok(RunOutcome::Failed(exc.to_string()));
            }
            PoolError::Typing(msg) => return Ok(RunOutcome::Failed(msg)),
            PoolError::Timeout { timeout } => {
                format!("TimeoutError: execution exceeded {}s", timeout.as_secs())
            }
            PoolError::CpuLimit { limit, .. } => {
                format!("TimeoutError: CPU time limit of {}s exceeded", limit.as_secs())
            }
            PoolError::Crashed { cause, .. } => {
                format!("sandbox worker crashed ({cause:?}); session state was not saved")
            }
            other => {
                self.discard();
                return Err(anyhow::anyhow!("monty worker error: {other}"));
            }
        };
        self.discard();
        Ok(RunOutcome::Failed(message))
    }
}

fn is_resource_error(exc: &MontyException) -> bool {
    matches!(exc.exc_type(), ExcType::MemoryError | ExcType::TimeoutError)
}

fn is_current_dump(bytes: &[u8]) -> bool {
    bytes.starts_with(DUMP_MAGIC)
}

/// Formats a snippet's final value the way a Python REPL echoes it: `None`
/// prints nothing, everything else prints its `repr()`.
pub fn repl_echo(value: &MontyObject) -> Option<String> {
    if *value == MontyObject::none() {
        None
    } else {
        Some(value.py_repr())
    }
}
