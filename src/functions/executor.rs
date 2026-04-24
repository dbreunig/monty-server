use anyhow::{Context, Result};
use monty::{
    ExtFunctionResult, LimitedTracker, MontyObject, MontyRepl, PrintStream, PrintWriter,
    ReplProgress, ReplStartError,
};
use monty::fs::{MountMode, MountTable};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::Instant;

use crate::envs::executor::ExecLimits;
use crate::envs::locks::LockRegistry;
use crate::envs::storage::EnvStorage;
use crate::functions::signature::HandlerSignature;

#[derive(Debug, Serialize)]
pub struct CallOutcome {
    pub status: u16,
    pub response_json: Option<String>, // serialized JSON of handler's return value
    pub error: Option<String>,         // handler/engine error message
    pub duration_ms: u64,
}

pub struct FunctionExecutor {
    pub storage: EnvStorage,
    pub locks: LockRegistry,
    pub limits: ExecLimits,
}

impl FunctionExecutor {
    pub fn new(storage: EnvStorage, locks: LockRegistry) -> Self {
        Self { storage, locks, limits: ExecLimits::default() }
    }

    pub async fn call_stateless(
        &self,
        code: &str,
        _sig: &HandlerSignature,
        kwargs: Map<String, Value>,
    ) -> Result<CallOutcome> {
        let limits = self.limits.clone();
        let code = code.to_string();
        let started = Instant::now();
        let outcome = tokio::task::spawn_blocking(move || {
            let repl = MontyRepl::new("<fn>", limits.tracker());
            run_handler(repl, &code, &kwargs, &PathBuf::new(), &limits)
        })
        .await
        .context("spawn_blocking panicked")??;
        Ok(CallOutcome { duration_ms: started.elapsed().as_millis() as u64, ..outcome })
    }
}

fn run_handler(
    repl: MontyRepl<LimitedTracker>,
    code: &str,
    kwargs: &Map<String, Value>,
    files_dir: &PathBuf,
    limits: &ExecLimits,
) -> Result<CallOutcome> {
    use base64::Engine;
    let kwargs_json = serde_json::to_string(kwargs).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD.encode(kwargs_json.as_bytes());
    let combined = format!(
        "{code}\n\
         import json, base64\n\
         __mk_kwargs = json.loads(base64.b64decode('{b64}').decode('utf-8'))\n\
         __result = handler(**__mk_kwargs)\n\
         __result\n"
    );
    let mut mount_table = MountTable::new();
    if !files_dir.as_os_str().is_empty() {
        mount_table
            .mount("/mnt", files_dir, MountMode::ReadWrite, limits.max_write_bytes)
            .context("mount files dir")?;
    }
    let mut streams: Vec<(PrintStream, String)> = Vec::new();
    let writer = PrintWriter::CollectStreams(&mut streams);
    let progress = match repl.feed_start(&combined, Vec::new(), writer) {
        Ok(p) => p,
        Err(err) => {
            let ReplStartError { repl: _, error } = *err;
            return Ok(CallOutcome {
                status: 500,
                response_json: None,
                error: Some(format!("{error}")),
                duration_ms: 0,
            });
        }
    };
    drive(progress, &mut mount_table)
}

fn drive(
    mut progress: ReplProgress<LimitedTracker>,
    mount_table: &mut MountTable,
) -> Result<CallOutcome> {
    loop {
        progress = match progress {
            ReplProgress::Complete { repl: _, value } => {
                let json = monty_object_to_json(&value)?;
                return Ok(CallOutcome {
                    status: 200,
                    response_json: Some(json),
                    error: None,
                    duration_ms: 0,
                });
            }
            ReplProgress::OsCall(call) => {
                let mut sink: Vec<(PrintStream, String)> = Vec::new();
                let writer = PrintWriter::CollectStreams(&mut sink);
                let result = match mount_table.handle_os_call(call.function, &call.args, &call.kwargs) {
                    Some(Ok(obj)) => ExtFunctionResult::Return(obj),
                    Some(Err(e)) => ExtFunctionResult::Error(e.into_exception()),
                    None => ExtFunctionResult::NotFound(format!("{:?}", call.function)),
                };
                match call.resume(result, writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl: _, error } = *err;
                        return Ok(CallOutcome {
                            status: 500,
                            response_json: None,
                            error: Some(format!("{error}")),
                            duration_ms: 0,
                        });
                    }
                }
            }
            ReplProgress::FunctionCall(call) => {
                let mut sink: Vec<(PrintStream, String)> = Vec::new();
                let writer = PrintWriter::CollectStreams(&mut sink);
                let name = call.function_name.clone();
                match call.resume(ExtFunctionResult::NotFound(name), writer) {
                    Ok(p) => p,
                    Err(err) => {
                        let ReplStartError { repl: _, error } = *err;
                        return Ok(CallOutcome {
                            status: 500,
                            response_json: None,
                            error: Some(format!("{error}")),
                            duration_ms: 0,
                        });
                    }
                }
            }
            ReplProgress::NameLookup(_) | ReplProgress::ResolveFutures(_) => {
                return Ok(CallOutcome {
                    status: 500,
                    response_json: None,
                    error: Some("unsupported progress state".into()),
                    duration_ms: 0,
                });
            }
        };
    }
}

fn monty_object_to_json(_obj: &MontyObject) -> Result<String> {
    // Placeholder: serialize via Debug for v1; Task 9b replaces with real conversion.
    Ok(format!("{_obj:?}"))
}
