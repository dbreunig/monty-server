use anyhow::{Context, Result};
use monty_types::MontyObject;
use serde::Serialize;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Instant;

use crate::envs::locks::LockRegistry;
use crate::envs::storage::EnvStorage;
use crate::functions::signature::HandlerSignature;
use crate::sandbox::{Restored, RunOutcome, Sandbox};

/// Sandbox global carrying the request body as a JSON string.
const KWARGS_INPUT: &str = "__mk_kwargs_json";

#[derive(Debug, Serialize)]
pub struct CallOutcome {
    pub status: u16,
    pub response_json: Option<String>, // serialized JSON of handler's return value
    pub error: Option<String>,         // handler/engine error message
    pub duration_ms: u64,
}

impl CallOutcome {
    fn failed(error: String) -> Self {
        Self {
            status: 500,
            response_json: None,
            error: Some(error),
            duration_ms: 0,
        }
    }
}

pub struct FunctionExecutor {
    pub storage: EnvStorage,
    pub locks: LockRegistry,
    sandbox: Arc<Sandbox>,
}

impl FunctionExecutor {
    pub fn new(storage: EnvStorage, locks: LockRegistry, sandbox: Arc<Sandbox>) -> Self {
        Self {
            storage,
            locks,
            sandbox,
        }
    }

    pub async fn call_stateless(
        &self,
        code: &str,
        _sig: &HandlerSignature,
        kwargs: Map<String, Value>,
    ) -> Result<CallOutcome> {
        let started = Instant::now();
        let session = self.sandbox.session("handler.py").await?;
        let outcome = run_handler(session, code, &kwargs, Vec::new()).await?;
        Ok(CallOutcome {
            duration_ms: started.elapsed().as_millis() as u64,
            ..outcome
        })
    }

    /// Runs the handler against a copy of the env's state. The env's files
    /// are mounted read-only and the session is never dumped, so nothing the
    /// handler does persists.
    pub async fn call_bound(
        &self,
        env_id: &str,
        code: &str,
        _sig: &HandlerSignature,
        kwargs: Map<String, Value>,
    ) -> Result<CallOutcome> {
        let started = Instant::now();
        // Snapshot writes are atomic renames, so holding the lock just for the
        // read is enough; bound calls never write back.
        let snapshot = {
            let _guard = self.locks.acquire(env_id).await;
            self.storage
                .read_snapshot(env_id)
                .await
                .context("read snapshot")?
        };
        let files_dir = self.storage.files_dir(env_id)?;
        let mount = || self.sandbox.mount(&files_dir, false);

        let mut session = self.sandbox.session("handler.py").await?;
        let outcome = match session.restore(snapshot, vec![mount()?]).await? {
            Restored::Empty | Restored::Loaded => {
                run_handler(session, code, &kwargs, vec![mount()?]).await?
            }
            Restored::Legacy => CallOutcome::failed(
                "the bound env's state predates the current Monty version; \
                 run any code in the env to reset it"
                    .into(),
            ),
            Restored::Failed(error) => CallOutcome::failed(error),
        };
        Ok(CallOutcome {
            duration_ms: started.elapsed().as_millis() as u64,
            ..outcome
        })
    }
}

async fn run_handler(
    mut session: crate::sandbox::Session,
    code: &str,
    kwargs: &Map<String, Value>,
    mounts: Vec<monty_pool::MountSpec>,
) -> Result<CallOutcome> {
    let kwargs_json = serde_json::to_string(kwargs).context("encode kwargs")?;
    // The handler's return value is JSON-encoded inside the sandbox, so its
    // wire form follows Python's `json.dumps` (tuples become arrays,
    // non-serializable values raise `TypeError`).
    let combined = format!(
        "{code}\n\
         import json as __mk_json\n\
         __mk_json.dumps(handler(**__mk_json.loads({KWARGS_INPUT})))\n"
    );
    let inputs = vec![(KWARGS_INPUT.to_owned(), MontyObject::string(kwargs_json))];
    let outcome = session.run(&combined, inputs, mounts).await?;
    session.finish().await;
    Ok(match outcome {
        RunOutcome::Complete(value) => match value.as_ref().as_str().map(normalize_json) {
            Some(Ok(json)) => CallOutcome {
                status: 200,
                response_json: Some(json),
                error: None,
                duration_ms: 0,
            },
            Some(Err(e)) => CallOutcome::failed(format!("handler returned invalid JSON: {e}")),
            None => CallOutcome::failed("handler result was not encoded as a string".into()),
        },
        RunOutcome::Failed(error) => CallOutcome::failed(error),
    })
}

/// Re-encodes Python's `json.dumps` output compactly. This also rejects the
/// `NaN`/`Infinity` tokens Python emits but JSON doesn't allow.
fn normalize_json(text: &str) -> Result<String, serde_json::Error> {
    let value: Value = serde_json::from_str(text)?;
    serde_json::to_string(&value)
}
